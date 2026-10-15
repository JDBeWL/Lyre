//! 搜索框，全局只有顶栏这一份输入状态。
//! 根视图的裸字母快捷键要 `&& !Input` 避让 `Input` 的 key context，否则打字会触发播放快捷键。

use std::time::Duration;

use crate::entities::LibraryEntity;
use crate::platform::*;
use crate::theme;

/// 攒 120ms 再提交：低于 80ms 等于没防抖，高于 200ms 列表明显迟钝。
const DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug)]
pub enum SearchBoxEvent {
    /// 用户开始打字：主区可能停在任意一页，靠这个事件把用户带到能看见结果的那页。
    /// 不带载荷：查询词只存在 `LibraryEntity::query` 里。
    Typing,
    /// 框获得焦点。点它就该切到搜索页，否则点了界面没反应。
    Focused,
}

pub struct SearchBox {
    input: Entity<InputState>,
    library: Entity<LibraryEntity>,
    /// 每次提交递增；防抖任务只在仍是当前代时才提交，避免把中间态提交上去。
    generation: u64,
    /// 最后一次由这个框提交给曲库的文字。
    submitted: String,
    _sub: Subscription,
    _mirror: Subscription,
}

impl EventEmitter<SearchBoxEvent> for SearchBox {}

impl SearchBox {
    pub fn new(
        window: &mut Window,
        library: Entity<LibraryEntity>,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("搜索标题、艺术家或专辑")
                .clean_on_escape()
        });
        let sub = cx.subscribe_in(&input, window, on_input_event);
        let mirror = cx.observe_in(&library, window, |this, _, window, cx| {
            this.mirror_query(window, cx);
        });

        Self {
            input,
            library,
            generation: 0,
            submitted: String::new(),
            _sub: sub,
            _mirror: mirror,
        }
    }

    /// 别处把查询词清掉后框里要跟上，否则留着的是一个已经不存在的过滤。
    /// 只镜像不是我们提交的那份：防抖的 120ms 里用户可能又打了字，旧值盖掉新值等于吃掉 keystroke。
    fn mirror_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.library.read(cx).query().to_string();
        if query == self.submitted {
            return;
        }
        // 在飞的防抖代要作废，否则它会把刚镜像进去的文字当成旧值再提交一次。
        self.generation = self.generation.wrapping_add(1);
        query.clone_into(&mut self.submitted);
        // `set_value` 刻意不发 `InputEvent::Change`，所以这里不会被当成「用户在打字」。
        self.input.update(cx, |input, cx| {
            input.set_value(query.as_str(), window, cx);
        });
    }

    /// 聚焦搜索框。`/` 与 `Ctrl+K` 快捷键、以及点框都走这里。
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// 清空输入框与查询。`Esc` 在输入框外按下时走这里。
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.generation = self.generation.wrapping_add(1);
        self.submitted.clear();
        self.library.update(cx, LibraryEntity::clear_query);
        cx.notify();
    }

    fn schedule_submit(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let executor = cx.background_executor().clone();

        cx.spawn(async move |this, cx| {
            executor.timer(DEBOUNCE).await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    let query = this.input.read(cx).value().to_string();
                    query.clone_into(&mut this.submitted);
                    this.library
                        .update(cx, |library, cx| library.set_query(query, cx));
                }
            })
            .ok();
        })
        .detach();
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // 作废旧的防抖代，否则它随后会把旧值再提交一次。
        self.generation = self.generation.wrapping_add(1);
        let query = self.input.read(cx).value().to_string();
        query.clone_into(&mut self.submitted);
        self.library
            .update(cx, |library, cx| library.set_query(query, cx));
        cx.notify();
    }
}

fn on_input_event(
    this: &mut SearchBox,
    _source: &Entity<InputState>,
    event: &InputEvent,
    window: &mut Window,
    cx: &mut Context<SearchBox>,
) {
    match event {
        InputEvent::Change => {
            // 顺序有意义：切页要立刻发生，查询提交走 120ms 防抖；反过来切页会晚 120ms。
            cx.emit(SearchBoxEvent::Typing);
            this.schedule_submit(cx);
        }
        InputEvent::PressEnter { .. } => {
            this.submit(window, cx);
        }
        InputEvent::Focus => {
            // 聚焦要同时重绘与切页，两个通知都发。
            cx.notify();
            cx.emit(SearchBoxEvent::Focused);
        }
        InputEvent::Blur => cx.notify(),
    }
}

impl Render for SearchBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_text = !self.input.read(cx).value().is_empty();
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);

        let border_color = if focused {
            theme::control()
        } else {
            theme::transparent()
        };

        // 左内边距必须恒定，否则打进第一个字符的瞬间整段文本右跳；需要让位的只有右端的清空按钮。
        let input_left = theme::GAP_MD;
        let input_right = if has_text {
            theme::GAP_XL
        } else {
            theme::GAP_MD
        };

        let mut root = div()
            .flex_none()
            .relative()
            // 跟着顶栏的中组收缩，`SEARCH_W` 只是上限：定宽会在窄窗口里把窗口控件挤出边界。
            .w_full()
            .max_w(px(theme::SEARCH_W))
            .h(px(theme::SEARCH_H))
            .flex()
            .items_center()
            .rounded(px(theme::RADIUS_PILL))
            .bg(theme::elevated())
            .border(px(0.5))
            .border_color(border_color)
            .hover(|style| style.bg(theme::border()))
            // `Input` 只有 32px 高、左右还各让出 12px 内边距，点这些空白不会聚焦；
            // 整条胶囊转发一次焦点，点哪儿都算点进搜索框（点在 `Input` 内部时这次 `focus` 幂等）。
            .cursor_text()
            .on_mouse_down(MouseButton::Left, {
                let input = self.input.clone();
                move |_, window, cx| {
                    input.update(cx, |input, cx| input.focus(window, cx));
                }
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    // `Input` 是组件库写死的 32px 定高元素，在 36px 的壳里要显式居中。
                    .flex()
                    .items_center()
                    .pl(px(input_left))
                    .pr(px(input_right))
                    .child(
                        Input::new(&self.input)
                            .appearance(false)
                            // 组件库的清空按钮出现时机与配色不受本项目控制。
                            .cleanable(false),
                    ),
            );

        if has_text {
            root = root.child(
                div()
                    .id("search-clear")
                    .absolute()
                    .right(px(theme::GAP_XS))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_color(theme::text_faint())
                    .hover(|style| style.text_color(theme::text()))
                    .tooltip(crate::common::tooltip::builder("清空搜索"))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.clear(window, cx);
                    }))
                    // 图标继承父级文字色，hover 变色才能作用在它身上。
                    .child(crate::icon::icon_inherit(crate::icon::Glyph::Cross, 14.)),
            );
        }

        root
    }
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`platform::*` 里的 gpui `test` 过程宏会遮蔽内置 `#[test]`。

    /// 搜索框底色与组件库暗色主题下 `Input` 文字色的对比度。
    /// 文字色来自组件库全局态而不是 `crate::theme`；`platform::init` 被切回亮色时输入文字会隐形。
    #[test]
    fn input_text_reads_against_the_search_pill() {
        use crate::platform::{Rgba, rgb};
        use crate::theme;

        // theme.rs 里那份 WCAG 计算是 `#[cfg(test)]` 私有函数，跨不到这里。
        fn luminance(c: Rgba) -> f32 {
            fn ch(v: f32) -> f32 {
                if v <= 0.039_28 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            }
            0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
        }
        fn over(fg: Rgba, bg: Rgba) -> Rgba {
            Rgba {
                r: fg.r * fg.a + bg.r * (1.0 - fg.a),
                g: fg.g * fg.a + bg.g * (1.0 - fg.a),
                b: fg.b * fg.a + bg.b * (1.0 - fg.a),
                a: 1.0,
            }
        }
        fn ratio(fg: Rgba, bg: Rgba) -> f32 {
            let base = theme::bg();
            let bg = over(bg, base);
            let fg = over(fg, bg);
            let (l1, l2) = (luminance(fg), luminance(bg));
            let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
            (hi + 0.05) / (lo + 0.05)
        }

        let pill = theme::elevated();

        // 组件库暗色主题下 `Input` 的文字色是 `neutral-50`（`#fafafa`）。
        let input_text_dark = rgb(0xfafafa);
        assert!(
            ratio(input_text_dark, pill) >= 4.5,
            "组件库暗色下的输入文字对搜索框底色只有 {:.2}:1，低于 AA ——\
             说明 platform::init 没有把组件库切到 ThemeMode::Dark",
            ratio(input_text_dark, pill)
        );

        // 反向锁定亮色文字色在这个底上确实不达标。
        let input_text_light = rgb(0x0a0a0a);
        assert!(
            ratio(input_text_light, pill) < 4.5,
            "近黑文字压在 {} 上竟然过了 AA（{:.2}:1）——\
             如果这是真的，说明搜索框底色被调亮了，\
             platform::init 里那次 ThemeMode::Dark 的必要性需要重新评估",
            "#282828",
            ratio(input_text_light, pill)
        );
    }
}
