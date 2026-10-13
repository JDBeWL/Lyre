//! 返回 / 前进的落点管理与主区左上角的浮动返回按钮。
//! [`HEADER_BACK_INSET`] 由 [`BACK_BTN`] 算出，各页头都靠它让位。

use crate::entities::{LibraryEntity, Scope};
use crate::icon::{Glyph, icon};
use crate::platform::*;
use crate::shell::{NavSection, ShellEntity};
use crate::theme;

use super::RootView;

impl RootView {
    /// 进入详情的入口有多个，落点由列表统一 emit；各入口各记一遍会漏。
    pub(super) fn record_return_point(&mut self, cx: &mut Context<Self>) {
        let from = self.shell.read(cx).nav_section();
        self.shell
            .update(cx, |shell, cx| shell.push_history(from, Scope::All, cx));
    }

    /// 返回 `false` 表示历史栈为空、什么都没做，调用方据此试下一优先级。
    fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.shell.read(cx).can_go_back() {
            return false;
        }
        navigate(&self.shell, &self.library, NavDirection::Back, cx);
        true
    }

    // 前进没有对称的方法：鼠标回调只拿得到 &mut App，只能走自由函数 `nav_click`

    /// Esc 一次只退一步，优先级：歌词遮罩 → 返回 → 收起右面板 → 清空搜索词。
    pub(super) fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 遮罩在最上层，必须先关，否则这次 Esc 会去退导航
        if self.shell.read(cx).shows_lyrics() {
            self.shell.update(cx, ShellEntity::toggle_lyrics);
            return;
        }

        // 判据是历史栈而不是 is_scoped()：栈空 + 作用域收窄只可能是点侧栏「曲目」进来的，确实没有上一步
        if self.go_back(cx) {
            return;
        }

        let panel_open = self.shell.read(cx).right_panel().is_some();
        if panel_open {
            if let Some(tab) = self.shell.read(cx).right_panel() {
                self.shell
                    .update(cx, |shell, cx| shell.toggle_panel(tab, cx));
            }
            return;
        }

        // 走搜索框而不是直接清曲库，否则框里留着旧文字
        // 判据用 has_query()：is_filtering() 在「只进了专辑没搜索」时也是 true，那一步会静默无效
        if self.library.read(cx).has_query() {
            self.search
                .update(cx, |search, cx| search.clear(window, cx));
            return;
        }

        // 词清空后再按一次才离开搜索页。落点固定是曲目列表：回「上一页」可能撞上被搜索改过的网格状态
        if self.shell.read(cx).nav_section() == NavSection::Search {
            self.shell.update(cx, |shell, cx| {
                shell.clear_history();
                shell.set_nav(NavSection::Tracks, cx);
            });
        }
    }
}

/// 悬停提示说退到哪去：取历史栈顶，不是当前位置。
pub(super) fn back_tooltip(spot: Option<(NavSection, Option<String>)>) -> String {
    match spot {
        Some((_, Some(label))) => format!("返回「{label}」"),
        Some((NavSection::Albums, None)) => "返回专辑".to_owned(),
        Some((NavSection::Artists, None)) => "返回艺术家".to_owned(),
        Some((NavSection::Home, None)) => "返回首页".to_owned(),
        Some((NavSection::Search, None)) => "返回搜索".to_owned(),
        Some((NavSection::Settings, None)) => "返回设置".to_owned(),
        // 曲目列表，或栈空（此时按钮不画，走不到这一支）
        _ => "返回曲目列表".to_owned(),
    }
}

/// 主区左上角的浮动返回按钮，直径 [`BACK_BTN`]、顶端偏移 [`BACK_INSET_TOP`]。
/// `hint` 只进悬停提示与无障碍名称，不画在按钮上。
pub(super) fn floating_back(
    hint: SharedString,
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
) -> impl IntoElement {
    let on_click = nav_click(shell, library, NavDirection::Back);

    div()
        .id(FLOATING_BACK_ID)
        .absolute()
        .left(px(theme::GAP_LG))
        .top(px(BACK_INSET_TOP))
        .size(px(BACK_BTN))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(rgba(0x000000d9))
        .text_color(theme::text())
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0x000000ff)))
        .on_click(on_click)
        .child(icon(Glyph::ArrowLeft, 20., theme::text()))
        .tooltip(crate::common::tooltip::builder(hint))
}

/// 后退与前进只差「从哪条栈取落点」。
#[derive(Clone, Copy)]
pub enum NavDirection {
    Back,
    Forward,
}

/// 后退 / 前进的实际搬运：取当前位置 → 从对应栈弹出落点 → 复原作用域并切区段。
pub fn navigate(
    shell: &Entity<ShellEntity>,
    library: &Entity<LibraryEntity>,
    direction: NavDirection,
    cx: &mut App,
) {
    let here = {
        let section = shell.read(cx).nav_section();
        let scope = library.read(cx).scope().clone();
        (section, scope)
    };

    let popped = shell.update(cx, |shell, _| match direction {
        NavDirection::Back => shell.pop_history(here),
        NavDirection::Forward => shell.pop_forward(here),
    });

    let Some((section, scope)) = popped else {
        return;
    };
    library.update(cx, |library, cx| library.restore_scope(scope, cx));
    shell.update(cx, |shell, cx| shell.set_nav(section, cx));
}

/// 设置页开关。进入时压的落点带着当前作用域，所以「从某张专辑进设置」出来还在那张专辑。
/// 已在设置页时按 = 后退一步；进设置必然先压了落点，走不到空栈。
pub fn toggle_settings(shell: &Entity<ShellEntity>, library: &Entity<LibraryEntity>, cx: &mut App) {
    if shell.read(cx).nav_section() == NavSection::Settings {
        navigate(shell, library, NavDirection::Back, cx);
        return;
    }
    let spot = (
        shell.read(cx).nav_section(),
        library.read(cx).scope().clone(),
    );
    shell.update(cx, |shell, cx| {
        shell.push_history(spot.0, spot.1, cx);
        shell.set_nav(NavSection::Settings, cx);
    });
}

/// 「点击即导航」的回调，浮动返回按钮与顶栏的前进 / 后退按钮共用。
pub fn nav_click(
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
    direction: NavDirection,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
        navigate(&shell, &library, direction, cx);
    }
}

/// 浮动返回按钮的直径，[`HEADER_BACK_INSET`] 由它算出。
pub const BACK_BTN: f32 = 40.0;
/// 按钮离主区卡片顶端的距离。详情类页头的大标题默认落在色带下沿；只有标题的页头（设置）把标题行顶到这一档、给到 [`BACK_BTN`] 等高。
pub const BACK_INSET_TOP: f32 = theme::GAP_MD;
/// 页头为了让开返回按钮额外增加的左内边距。
pub const HEADER_BACK_INSET: f32 = BACK_BTN + theme::GAP_LG;
/// 浮动返回按钮的元素 id。
const FLOATING_BACK_ID: &str = "floating-back";

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：glob 会把 `crate::platform::*` 里的 gpui `test` 过程宏带进来，遮蔽内置 `#[test]`
    use super::back_tooltip;
    use crate::shell::NavSection;

    #[test]
    fn back_tooltip_names_the_destination_not_the_current_place() {
        assert_eq!(back_tooltip(Some((NavSection::Albums, None))), "返回专辑");

        assert_eq!(
            back_tooltip(Some((NavSection::Tracks, Some("城市漫游".to_owned())))),
            "返回「城市漫游」"
        );

        for spot in [
            None,
            Some((NavSection::Home, None)),
            Some((NavSection::Tracks, None)),
            Some((NavSection::Albums, None)),
            Some((NavSection::Artists, None)),
            Some((NavSection::Search, None)),
            Some((NavSection::Settings, None)),
        ] {
            assert!(
                !back_tooltip(spot.clone()).trim().is_empty(),
                "落点 {spot:?} 的返回提示是空的"
            );
        }
    }
}
