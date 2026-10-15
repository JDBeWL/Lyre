//! 首页「浏览」那排跳转卡。
//! 不压历史栈：历史栈只记「作用域收窄」，否则返回会退到一半变成切区段。

use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::shell::{NavSection, ShellEntity};
use crate::theme;

const ENTRY_H: f32 = 72.0;
/// 240 容纳「图标 40 + 间距 12 + 两行文字」；三张卡加两条间距共 744px。
/// 这是上限而不是定宽：宽度不够时三张均分收缩，560px 窗口下每张约 139px。
const ENTRY_W: f32 = 240.0;

/// 一张跳转卡片：图标 + 名称 + 数量，点击切到对应区段。
pub(super) fn entry_card(
    id: &'static str,
    glyph: Glyph,
    title: &'static str,
    count: String,
    section: NavSection,
    shell: &Entity<ShellEntity>,
    library: &Entity<LibraryEntity>,
) -> AnyElement {
    let shell = shell.clone();
    let library = library.clone();

    div()
        .id(id)
        .flex_none()
        .h(px(ENTRY_H))
        .flex_1()
        .min_w_0()
        .max_w(px(ENTRY_W))
        .flex()
        .items_center()
        .gap(px(theme::GAP_MD))
        .px(px(theme::GAP_MD))
        .rounded(px(theme::RADIUS_CARD))
        .bg(theme::elevated())
        .cursor_pointer()
        .hover(|style| style.bg(theme::panel()))
        .on_click(move |_, _, cx| {
            // `go_to` 自己会退作用域并记一条历史，顶栏 ← 因此能退回首页。
            crate::shell::go_to(section, &shell, &library, cx);
        })
        .child(
            div()
                .flex_none()
                .size(px(40.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_LG))
                .bg(theme::panel())
                .text_color(theme::text())
                .child(icon_inherit(glyph, 20.)),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(theme::FONT_BODY))
                        .font_weight(theme::WEIGHT_SUBTITLE)
                        .text_color(theme::text())
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(theme::text_muted())
                        .child(count),
                ),
        )
        .into_any_element()
}
