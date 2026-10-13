//! 三种空状态：曲库本来就是空的、作用域里没有曲目、搜索没有匹配。

use crate::common::button::{ButtonKind, text_button};
use crate::entities::LibraryEntity;
use crate::platform::*;
use crate::theme;

/// 三种空状态共用的外框。
fn empty_frame() -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_start()
        .pt(px(theme::GAP_XL * 3.0))
        .px(px(theme::GAP_XL * 2.0))
        .gap(px(theme::GAP_MD))
}

fn empty_title(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_size(px(theme::FONT_DISPLAY))
        .font_weight(theme::WEIGHT_TITLE)
        .text_color(theme::text())
        .child(text.into())
}

/// 空状态的说明文字：限宽 460，否则会被拉成一整行长句。
fn empty_hint(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .max_w(px(460.))
        .text_size(px(theme::FONT_BODY))
        .text_color(theme::text_muted())
        .child(text.into())
}

/// 空状态：曲库本来就是空的。
pub(super) fn empty_library(library: Entity<LibraryEntity>) -> impl IntoElement {
    empty_frame()
        .child(empty_title("曲库还是空的"))
        .child(empty_hint(
            "把音频文件或文件夹拖进窗口，也可以点下面的按钮。\
             支持 MP3 / FLAC / M4A / WAV / OGG / Opus 等常见格式。",
        ))
        .child(div().mt(px(theme::GAP_SM)).child(text_button(
            "empty-import",
            "导入音乐",
            ButtonKind::Primary,
            move |_, _, cx| {
                library.update(cx, LibraryEntity::open_dialog);
            },
        )))
}

/// 收窄了作用域、但里面一首曲目都没有时的提示。出路是返回上一级，与 [`no_matches`] 的「清空搜索」不是一回事。
pub(super) fn empty_scope(
    library: Entity<LibraryEntity>,
    scope: Option<String>,
    from_list: bool,
) -> impl IntoElement {
    let name = scope.unwrap_or_else(|| "这里".to_owned());
    // 「可能已被移除」只对专辑 / 艺术家成立：歌单的成员不会随曲库消失，新歌单从来没装过东西
    let (title, hint) = if from_list {
        (
            format!("「{name}」还没有曲目"),
            "在任意曲目上右键，选「加入歌单」就能填进来。",
        )
    } else {
        (
            format!("「{name}」里还没有曲目"),
            "它们可能已经被移除。返回上一级可以继续浏览整个曲库。",
        )
    };
    empty_frame()
        .child(empty_title(title))
        .child(empty_hint(hint))
        .child(div().mt(px(theme::GAP_SM)).child(text_button(
            "exit-scope",
            "返回曲库",
            ButtonKind::Primary,
            move |_, _, cx| {
                library.update(cx, LibraryEntity::exit_scope);
            },
        )))
}

pub(super) fn no_matches(
    library: Entity<LibraryEntity>,
    total: usize,
    query: &str,
) -> impl IntoElement {
    empty_frame()
        .child(empty_title(format!("没有匹配「{query}」的曲目")))
        .child(empty_hint(format!(
            "曲库里有 {total} 首，试试别的关键词，或者清空搜索看全部。"
        )))
        .child(div().mt(px(theme::GAP_SM)).child(text_button(
            "clear-search",
            "清空搜索",
            ButtonKind::Primary,
            move |_, _, cx| {
                library.update(cx, LibraryEntity::clear_query);
            },
        )))
}
