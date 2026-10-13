//! 侧栏的导航项：首页 / 曲库，以及图标态下替代整个列表的那一项。
//! 都走 [`crate::shell::go_to`]：先记一条历史再跳，顶栏的 ← 因此能退回来源页。

use crate::common::tooltip;
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::shell::{NavSection, ShellEntity};
use crate::theme;

/// 导航项共用的依赖。
pub(super) struct NavDeps {
    pub(super) shell: Entity<ShellEntity>,
    pub(super) library: Entity<LibraryEntity>,
}

/// 「首页」项：记一条历史再跳，并退出作用域 ——
/// 带着作用域进首页会激活只有详情页头图才填得上的渐变色带和页头留白。
pub(super) fn home_item(
    id: &'static str,
    label: &'static str,
    show_label: bool,
    selected: bool,
    deps: NavDeps,
) -> AnyElement {
    let NavDeps { shell, library, .. } = deps;

    let mut item = home_item_shell(id, Glyph::Home, label, show_label, selected)
        .on_click(move |_, _, cx| crate::shell::go_to(NavSection::Home, &shell, &library, cx));

    if !show_label {
        item = item.tooltip(tooltip::builder(label));
    }
    item.into_any_element()
}

/// 「曲库」项：回到整个曲库（全部曲目），切到 `Tracks`。
pub(super) fn library_item(
    id: &'static str,
    label: &'static str,
    show_label: bool,
    selected: bool,
    deps: NavDeps,
) -> AnyElement {
    let NavDeps { shell, library, .. } = deps;

    let mut item = home_item_shell(id, Glyph::Folder, label, show_label, selected)
        .on_click(move |_, _, cx| crate::shell::go_to(NavSection::Tracks, &shell, &library, cx));

    if !show_label {
        item = item.tooltip(tooltip::builder(label));
    }
    item.into_any_element()
}

/// 「曲库」是否选中：在看曲库的切面，且没有停在某个具体条目里。
pub(super) fn library_item_selected(nav: NavSection, scoped: bool) -> bool {
    nav.in_library() && !scoped
}

/// 导航项共用的外壳：图标 + 文字 + 悬停 / 选中样式，不绑行为（点击由调用方接）。
fn home_item_shell(
    id: &'static str,
    glyph: Glyph,
    label: &'static str,
    show_label: bool,
    selected: bool,
) -> Stateful<Div> {
    let item = div()
        .id(id)
        .flex_none()
        .h(px(theme::SIDEBAR_ITEM_H))
        .flex()
        .items_center()
        .gap(px(theme::GAP_MD))
        .rounded(px(theme::RADIUS_PILL))
        .cursor_pointer()
        .px(px(theme::GAP_MD))
        .when(selected, |item| item.bg(theme::elevated()))
        .text_color(if selected {
            theme::text()
        } else {
            theme::text_muted()
        })
        .hover(|style| style.bg(theme::elevated()).text_color(theme::text()))
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(24.))
                .child(icon_inherit(glyph, 20.)),
        );

    if show_label {
        item.child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(theme::FONT_BODY))
                .font_weight(theme::WEIGHT_BUTTON)
                .truncate()
                .child(label),
        )
    } else {
        item
    }
}

/// 图标态下替代整个列表的那一项，点亮判据是 `scoped` 而不是 `nav != Tracks`：
/// 点侧栏条目后 `nav` 也是 `Tracks`，用视图当代理会漏判。
pub(super) fn lib_fallback_item(scoped: bool, deps: NavDeps) -> AnyElement {
    let NavDeps { shell, library, .. } = deps;

    div()
        .id("side-lib-fallback")
        .flex_none()
        .h(px(theme::SIDEBAR_ITEM_H))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_PILL))
        .cursor_pointer()
        .text_color(if scoped {
            theme::text()
        } else {
            theme::text_muted()
        })
        .hover(|style| style.bg(theme::elevated()))
        .on_click(move |_, _, cx| crate::shell::go_to(NavSection::Tracks, &shell, &library, cx))
        .child(icon_inherit(Glyph::Folder, 20.))
        // 与展开态的「曲库」项同名。
        .tooltip(tooltip::builder("曲库"))
        .into_any_element()
}
