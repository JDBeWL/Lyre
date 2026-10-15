//! 顶部栏，兼任自绘的窗口标题栏：导航 + 居中搜索 + 设置入口 + 窗口控件。
//! 命中测试取第一个命中的条目，所以拖拽区不能包住任何可点击元素 —— 窗口控件必须是它的兄弟节点。
//! 左右两组各取 `flex_1` 且都不写 `min_w_0`：一边定宽搜索框就不在正中，塌成 0 会把关闭键推出窗口。

use crate::components::search_box::SearchBox;
use crate::components::window_controls;
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::root::{NavDirection, nav_click, toggle_settings};
use crate::shell::{NavSection, ShellEntity, SidebarState};
use crate::theme;

pub struct TopBar {
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
    search: Entity<SearchBox>,
}

impl TopBar {
    pub fn new(
        shell: Entity<ShellEntity>,
        library: Entity<LibraryEntity>,
        search: Entity<SearchBox>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = cx;
        Self {
            shell,
            library,
            search,
        }
    }
}

impl Render for TopBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (can_back, can_forward) = {
            let shell = self.shell.read(cx);
            (shell.can_go_back(), shell.can_go_forward())
        };
        // 侧栏折叠进图标态 / 隐藏态后，侧栏内那颗开关就不画了；这颗常驻是唯一的展开入口。
        let sidebar_expanded = self.shell.read(cx).sidebar() == SidebarState::Expanded;
        let settings_active = self.shell.read(cx).nav_section() == NavSection::Settings;
        let side = side_min();

        div()
            .flex_none()
            .h(px(theme::TOP_BAR_H))
            .flex()
            .items_center()
            .bg(theme::chrome())
            // 左组余量是拖拽区，与右组同为 `flex_1` 才让搜索框居中。
            .child(
                div()
                    .flex_1()
                    .min_w(px(side))
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(theme::GAP_SM))
                    .pl(px(theme::GAP_LG))
                    .child(nav_button(
                        "top-sidebar",
                        if sidebar_expanded {
                            Glyph::PanelLeft
                        } else {
                            Glyph::PanelLeftOpen
                        },
                        if sidebar_expanded {
                            "折叠侧栏"
                        } else {
                            "展开侧栏"
                        },
                        Some(Box::new({
                            let shell = self.shell.clone();
                            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                shell.update(cx, ShellEntity::toggle_sidebar);
                            }
                        }) as NavClick),
                    ))
                    // 后退 / 前进两颗常驻可见（禁用只让图标变淡）：按钮出现/消失会让搜索框横跳。
                    .child(nav_button(
                        "top-back",
                        Glyph::ArrowLeft,
                        "后退",
                        can_back.then(|| {
                            Box::new(nav_click(
                                self.shell.clone(),
                                self.library.clone(),
                                NavDirection::Back,
                            )) as NavClick
                        }),
                    ))
                    .child(nav_button(
                        "top-forward",
                        Glyph::ArrowRight,
                        "前进",
                        can_forward.then(|| {
                            Box::new(nav_click(
                                self.shell.clone(),
                                self.library.clone(),
                                NavDirection::Forward,
                            )) as NavClick
                        }),
                    ))
                    .child(drag_area()),
            )
            // 中组：搜索框。顶栏唯一能让步的一块 —— 定宽会把两组按钮挤出窗口，关闭键就点不到了。
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(theme::GAP_MD))
                    .child(self.search.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(side))
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(theme::GAP_SM))
                    .pr(px(theme::GAP_LG))
                    .child(drag_area())
                    .child(settings_button(
                        self.shell.clone(),
                        self.library.clone(),
                        settings_active,
                    ))
                    .child(window_controls::window_controls(window)),
            )
    }
}

/// 拖拽区：只登记命中盒、不绑监听器。
/// 双击留给 `DefWindowProc` 做最大化 ⇄ 还原；自己接 `zoom_window` 只最大化、不切换，会和系统打架。
fn drag_area() -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .window_control_area(WindowControlArea::Drag)
}

/// 顶栏的导航圆按钮。
/// `on_click` 用 `Option` 而不是 `enabled` 标志：禁用态要真的解绑点击，只改颜色会留下按不动的按钮。
fn nav_button(
    id: &'static str,
    glyph: Glyph,
    label: &'static str,
    on_click: Option<NavClick>,
) -> AnyElement {
    let enabled = on_click.is_some();

    let mut button = div()
        .id(id)
        .flex_none()
        .size(px(TOP_NAV_BTN))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme::elevated())
        .text_color(if enabled {
            theme::text()
        } else {
            theme::text_faint()
        })
        .child(icon_inherit(glyph, 18.));

    if let Some(on_click) = on_click {
        button = button
            .cursor_pointer()
            .hover(|style| style.bg(theme::hover()))
            .on_click(on_click);
    }

    button
        .tooltip(crate::common::tooltip::builder(label))
        .into_any_element()
}

type NavClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// 设置入口：右端那颗齿轮。
fn settings_button(
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
    active: bool,
) -> AnyElement {
    div()
        .id("top-settings")
        .flex_none()
        .size(px(TRIGGER_PX))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .text_color(if active {
            theme::text()
        } else {
            theme::text_muted()
        })
        .hover(|style| style.bg(theme::elevated()).text_color(theme::text()))
        .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            toggle_settings(&shell, &library, cx);
        })
        .child(icon_inherit(Glyph::Settings, 18.))
        .tooltip(crate::common::tooltip::builder("设置"))
        .into_any_element()
}

const TOP_NAV_BTN: f32 = 32.0;
/// 设置入口与窗口控件的圆按钮直径。
const TRIGGER_PX: f32 = 32.0;
/// 左组的内容宽：三颗导航圆按钮 + 两道间距 + 左内边距。
const LEFT_MIN: f32 = 3.0 * TOP_NAV_BTN + 2.0 * theme::GAP_SM + theme::GAP_LG;
/// 窗口控件的内容宽：最小化 / 最大化 / 关闭三颗加两道间距。
const WINDOW_CTRLS_W: f32 = 3.0 * TRIGGER_PX + 2.0 * theme::GAP_XS;
/// 右组的内容宽：设置入口 + 窗口控件 + 与拖拽区之间的两道间距 + 右内边距。
fn right_min() -> f32 {
    TRIGGER_PX + WINDOW_CTRLS_W + 2.0 * theme::GAP_SM + theme::GAP_LG
}

/// 顶栏左右两组各自至少要多宽；两组等宽，中间的搜索框才落在正中。
/// 右组没有「放不下就收掉一颗」的退路：最小化 / 最大化 / 关闭必须永远点得到。
pub(crate) fn side_min() -> f32 {
    right_min().max(LEFT_MIN)
}

#[cfg(test)]
mod tests {
    use super::{LEFT_MIN, right_min, side_min};
    use crate::theme;

    /// `main.rs` 的 `window_min_size` 是 560，它约束的是客户区，也就是 `shell.width()`。
    const CLIENT_MIN: f32 = 560.0;
    /// 搜索框还能打字的下限。
    const SEARCH_ROOM_MIN: f32 = 160.0;

    /// 一侧占 `side` 时搜索框还剩多宽。三块都是 `flex_1`（basis 0）→ 先按窗口三等分；
    /// 三等分不够一侧的内容宽时，两侧各占内容宽，剩下的归中间。
    fn pill_width(window: f32, side: f32) -> f32 {
        let side = (window / 3.0).max(side);
        (window - 2.0 * side - 2.0 * theme::GAP_MD).max(0.0)
    }

    #[test]
    fn 顶栏在最小窗口也放得下() {
        for width in [CLIENT_MIN, 592.0, 648.0, 860.0, 1180.0, 1920.0] {
            assert!(
                pill_width(width, side_min()) >= SEARCH_ROOM_MIN,
                "{width}px 的窗口只剩 {:.0}px 给搜索框，低于下限 {SEARCH_ROOM_MIN}",
                pill_width(width, side_min())
            );
        }
        assert!(
            pill_width(1920.0, side_min()) >= theme::SEARCH_W,
            "最大化窗口都放不下定宽搜索框：{}",
            pill_width(1920.0, side_min())
        );
    }

    /// 留位按更宽的一组：右组一旦比左组窄，三颗导航按钮会被挤到搜索框那边。
    #[test]
    fn 留位按的是右组() {
        assert!(
            LEFT_MIN <= right_min(),
            "左组 {LEFT_MIN} 比右组 {} 还宽，等宽就要改按左组留位",
            right_min()
        );
    }
}
