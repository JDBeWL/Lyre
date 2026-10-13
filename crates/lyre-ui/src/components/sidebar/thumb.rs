//! 侧栏列表行首那一格，边长统一由 [`LIB_COVER`] 给出。

use crate::cover::CoverCache;
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::theme;

use super::entries::{SideEntry, SideScope};

/// 行内封面缩略图边长（行高 48 减上下各 8）。与 `entries.rs` 的 `LIB_ROW_H` 互相约束。
const LIB_COVER: f32 = 32.0;

/// 行首那一格：歌单走 [`crate::common::cover::list_cover`]，专辑 / 艺术家走 [`crate::common::cover::thumb`]，
/// 收藏与「新建」画图标。
pub(super) fn row_thumb(
    entry: &SideEntry,
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    cx: &mut App,
) -> AnyElement {
    if matches!(entry.scope, SideScope::Collection(_)) {
        return crate::common::cover::list_cover(
            covers,
            library,
            LIB_COVER,
            theme::RADIUS_SM,
            entry.cover_file.as_deref(),
            entry.cover_id,
            &entry.title,
            cx,
        );
    }

    match entry.id {
        // 艺术家行画正圆，专辑行画圆角矩形。
        Some(id) => {
            let artist_row = entry.scope == SideScope::Artist;
            let (radius, glyph) = if artist_row {
                (theme::RADIUS_PILL, Glyph::Artist)
            } else {
                (theme::RADIUS_SM, Glyph::Album)
            };
            crate::common::cover::thumb(covers, library, id, LIB_COVER, radius, glyph, cx)
        }
        None => glyph_thumb(entry.scope),
    }
}

/// 没有代表曲目时（收藏、空歌单、新建行）的行首方块：一块圆角底 + 一个图标。
fn glyph_thumb(scope: SideScope) -> AnyElement {
    let glyph = match scope {
        SideScope::Favorites => Glyph::HeartFilled,
        SideScope::NewCollection => Glyph::Plus,
        _ => Glyph::Music,
    };
    div()
        .flex_none()
        .size(px(LIB_COVER))
        .rounded(px(theme::RADIUS_SM))
        .bg(theme::elevated())
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme::text_faint())
        .child(icon_inherit(glyph, 16.))
        .into_any_element()
}
