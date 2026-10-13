//! 行内各格：勾选框、标题与副标题、专辑、行尾操作区。

use lyre_core::{Track, TrackId, UNKNOWN_ALBUM, UNKNOWN_ARTIST};

use crate::cover::CoverCache;
use crate::entities::{LibraryEntity, ScopeKind};
use crate::icon::{Glyph, icon, icon_inherit};
use crate::platform::*;
use crate::theme;

use super::row::RowState;
use super::{
    ACTIONS_W, ALBUM_W, CHECK_BOX, CHECK_W, RowEnv, THUMB, THUMB_COMPACT, TrackList, TrackListEvent,
};

/// 双行阈值：行高 >= 它才画两行。必须落在 [`Density`](crate::shell::Density) 的两档之间（紧凑 40px / 舒适 52px）。
const TWO_LINE_MIN: f32 = 48.0;

/// 勾选框：选中态只画在这里，不进 `RowState` 的底色。
pub(super) fn check_cell(
    index: usize,
    track: TrackId,
    picked: bool,
    list: WeakEntity<TrackList>,
) -> AnyElement {
    div()
        .id(("row-check", index))
        .flex_none()
        .w(px(CHECK_W))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .tooltip(crate::common::tooltip::builder(if picked {
            "取消选中"
        } else {
            "选中（Shift 连选）"
        }))
        .on_click(move |event, _window, cx| {
            let shift = event.modifiers().shift;
            list.update(cx, |list, cx| list.pick_row(index, track, shift, cx))
                .ok();
        })
        .child(
            div()
                .size(px(CHECK_BOX))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .border_1()
                .border_color(if picked {
                    theme::control()
                } else {
                    theme::text_faint()
                })
                .bg(if picked {
                    theme::control()
                } else {
                    theme::transparent()
                })
                .when(picked, |frame| {
                    frame.child(icon(Glyph::Check, 12., theme::text_on_control()))
                }),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn title_cell(
    index: usize,
    track: &Track,
    state: RowState,
    env: RowEnv,
    library: &Entity<LibraryEntity>,
    list: WeakEntity<TrackList>,
    covers: &Entity<CoverCache>,
    track_id: TrackId,
    cx: &mut App,
) -> impl IntoElement {
    // 40px 的行高塞不下两行，双行只在舒适密度出现
    let two_lines = env.row_height >= TWO_LINE_MIN;
    let artist = track.artist.clone().unwrap_or_default();
    let thumb_size = if two_lines { THUMB } else { THUMB_COMPACT };
    let cover = crate::common::cover::thumb(
        covers,
        library,
        track_id,
        thumb_size,
        theme::RADIUS_SM,
        Glyph::Music,
        cx,
    );

    let row_library = library.clone();
    let row_list = list.clone();
    let selectable = env.selectable;

    let mut cell = div()
        .id(("track-pick", index))
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .pr(px(theme::GAP_MD))
        .justify_center()
        .cursor_pointer()
        .on_click(move |event, _, cx| {
            if selectable {
                let shift = event.modifiers().shift;
                row_list
                    .update(cx, |list, cx| list.pick_row(index, track_id, shift, cx))
                    .ok();
            } else {
                row_library.update(cx, |library, cx| {
                    library.activate_in_view(track_id, cx);
                });
            }
        });

    let mut title_line = div()
        .flex()
        .items_center()
        .gap(px(theme::GAP_SM))
        .min_w_0()
        .child(
            div()
                .min_w_0()
                // 艺术家段是 `flex_shrink(2.0)`：文本过长时艺术家先被截断
                .flex_shrink(1.0)
                .truncate()
                .text_size(px(theme::FONT_TRACK))
                .font_weight(state.title_weight())
                .text_color(state.title_color())
                .child(track.title.clone()),
        );

    if two_lines {
        let segments = subtitle_segments(
            index,
            track.artist.clone(),
            track.album.clone(),
            env.kind,
            library.clone(),
            list,
        );
        cell = cell.child(title_line).child(
            div()
                .flex()
                .items_center()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(if state.is_playing() {
                    theme::accent_text()
                } else {
                    theme::text_muted()
                })
                .children(segments),
        );
    } else {
        // 紧凑密度只有一行：标题与艺术家挤在同一段宽度里，两边都必须能收缩，
        // 否则长艺术家名会画到专辑列上。艺术家 shrink 更大，先让位
        if !artist.is_empty() && env.kind != ScopeKind::Artist {
            title_line = title_line.child(
                div()
                    .min_w_0()
                    .flex_shrink(2.0)
                    .truncate()
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(if state.is_playing() {
                        theme::accent_text()
                    } else {
                        theme::text_muted()
                    })
                    .child(artist),
            );
        }
        cell = cell.child(title_line);
    }

    // 点击区只覆盖文字块，封面不可点
    div()
        .flex()
        .items_center()
        .gap(px(theme::GAP_MD))
        .flex_1()
        .min_w_0()
        .child(cover)
        .child(cell)
}

/// 副标题里该出现的可点片段：`艺术家 · 专辑`。缺一边时退化成单段，分隔符只在两段都在时才出现。
fn subtitle_segments(
    index: usize,
    artist: Option<String>,
    album: Option<String>,
    kind: ScopeKind,
    library: Entity<LibraryEntity>,
    list: WeakEntity<TrackList>,
) -> Vec<AnyElement> {
    // 「未知艺术家 / 未知专辑」这类占位标签不可点，点进去是空作用域
    let artist = artist.filter(|a| lyre_core::artist_key_str(Some(a)) != UNKNOWN_ARTIST);
    let album = album.filter(|a| lyre_core::album_key_str(Some(a)) != UNKNOWN_ALBUM);

    let (want_artist, want_album) = subtitle_parts(kind, artist.is_some(), album.is_some());
    let artist = artist.filter(|_| want_artist);
    let album = album.filter(|_| want_album);

    let both = artist.is_some() && album.is_some();
    let mut out = Vec::new();

    if let Some(name) = artist {
        // 元素 id 必须逐行唯一（用行号而不是文字）：重复的纯文字 id 会让 gpui 复用前一个元素的命中区域。
        // 文字照写整串（`甲/乙`），落点用拆分后的一位 —— 分组里没有整串，按它进作用域是空页
        let target = lyre_core::artist_key_str(Some(&name)).to_owned();
        out.push(subtitle_link(
            format!("subtitle-artist-{index}"),
            name.clone(),
            SubtitleTarget::Artist(target),
            library.clone(),
            list.clone(),
        ));

        if both {
            out.push(subtitle_sep());
        }
    }

    if let Some(title) = album {
        out.push(subtitle_link(
            format!("subtitle-album-{index}"),
            title.clone(),
            SubtitleTarget::Album(title),
            library,
            list,
        ));
    }

    out
}

/// gpui 的鼠标事件会冒泡到标题格的 `on_click`，必须 `stop_propagation`，
/// 否则一次点击既进详情又把这首送进播放器。
fn subtitle_link(
    id: String,
    text: String,
    target: SubtitleTarget,
    library: Entity<LibraryEntity>,
    list: WeakEntity<TrackList>,
) -> AnyElement {
    div()
        .id(SharedString::from(id))
        .min_w_0()
        .truncate()
        .cursor_pointer()
        .hover(|style| style.text_color(theme::text()).underline())
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            // 先记落点再收窄作用域，顺序不能反
            list.update(cx, |_, cx| cx.emit(TrackListEvent::RecordFromTrack))
                .ok();
            library.update(cx, |library, cx| match &target {
                SubtitleTarget::Artist(name) => library.enter_artist(name.clone(), cx),
                SubtitleTarget::Album(title) => library.enter_album(title.clone(), cx),
            });
        })
        .child(text)
        .into_any_element()
}

/// 副标题片段点进去之后要去哪。
#[derive(Clone)]
enum SubtitleTarget {
    Artist(String),
    Album(String),
}

/// 副标题该显示哪些段：`(艺术家?, 专辑?)`。详情页里不重复页头已有的那段。
fn subtitle_parts(kind: ScopeKind, has_artist: bool, has_album: bool) -> (bool, bool) {
    match kind {
        ScopeKind::All => (has_artist, has_album),
        ScopeKind::Artist => (false, has_album),
        ScopeKind::Album => (has_artist, false),
    }
}

/// 专辑列的一格，可点进这张专辑的详情页。
pub(super) fn album_cell(
    index: usize,
    album: Option<&str>,
    library: Entity<LibraryEntity>,
    list: WeakEntity<TrackList>,
) -> AnyElement {
    // 空 / 「未知专辑」不可点，点进去是空作用域
    let known = lyre_core::album_key_str(album) != UNKNOWN_ALBUM;
    let text = album.unwrap_or("—").to_owned();

    let mut cell = div()
        .flex_none()
        .w(px(ALBUM_W))
        .pr(px(theme::GAP_MD))
        .text_size(px(theme::FONT_CAPTION))
        .text_color(theme::text_muted());

    if known {
        // 闭包是 `move` 的，点击目标与显示各要一份。
        let target = text.clone();
        cell = cell.child(
            div()
                .id(("album-link", index))
                .min_w_0()
                .truncate()
                .cursor_pointer()
                .hover(|style| style.text_color(theme::text()).underline())
                .on_click(move |_, _, cx| {
                    list.update(cx, |_, cx| cx.emit(TrackListEvent::RecordFromTrack))
                        .ok();
                    library.update(cx, |library, cx| library.enter_album(target.clone(), cx));
                })
                .child(text),
        );
    } else {
        cell = cell.truncate().child(text);
    }
    cell.into_any_element()
}

fn subtitle_sep() -> AnyElement {
    div().child(" · ").into_any_element()
}

/// 行尾操作区：收藏（♡/♥）+ 更多（⋯）+ 移除（✕）。
/// 更多与移除悬停才浮现，显隐由 `group_hover` 驱动而不是视图状态。
#[allow(clippy::too_many_arguments)]
pub(super) fn actions_cell(
    index: usize,
    state: RowState,
    group: SharedString,
    track_id: TrackId,
    liked: bool,
    library: Entity<LibraryEntity>,
    list: WeakEntity<TrackList>,
    menu: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> impl IntoElement {
    let always_visible = state.is_playing();

    let round = |id: (&'static str, usize), visible: bool, group: SharedString| {
        div()
            .id(id)
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_PILL))
            .text_color(theme::text_faint())
            .cursor_pointer()
            .opacity(if visible { 1.0 } else { 0.0 })
            .when(!visible, |button| {
                button.group_hover(group, |style| style.opacity(1.0))
            })
    };

    // 已收藏时常驻显示（那是收藏唯一的记号）；未收藏时悬停才浮现
    let heart = round(
        ("track-like", index),
        liked || always_visible,
        group.clone(),
    )
    .when(liked, |button| button.text_color(theme::accent_text()))
    .hover(|style| {
        style.bg(theme::elevated()).text_color(if liked {
            theme::accent_hover()
        } else {
            theme::text()
        })
    })
    .tooltip(crate::common::tooltip::builder(if liked {
        "取消收藏"
    } else {
        "收藏"
    }))
    .on_click({
        let library = library.clone();
        move |_, _, cx| {
            library.update(cx, |library, cx| {
                library.toggle_like(track_id, cx);
            });
        }
    })
    .child(icon_inherit(
        if liked {
            Glyph::HeartFilled
        } else {
            Glyph::Heart
        },
        15.,
    ));

    let more = crate::common::menu::dropdown(
        round(("track-more", index), always_visible, group.clone())
            .hover(|style| style.bg(theme::elevated()).text_color(theme::text()))
            .tooltip(crate::common::tooltip::builder("更多操作"))
            .child(icon_inherit(Glyph::More, 15.)),
        // 行尾贴近面板右缘，菜单右对齐才不会探出窗口
        Anchor::TopRight,
        menu,
    );

    let remove = round(("track-remove", index), always_visible, group)
        .hover(|style| style.bg(theme::elevated()).text_color(theme::danger()))
        .tooltip(crate::common::tooltip::builder("从曲库移除"))
        .on_click(move |_, _, cx| {
            let removed = library.update(cx, |library, cx| library.remove(track_id, cx));
            if let Some((track, at)) = removed {
                list.update(cx, |_, cx| cx.emit(TrackListEvent::Removed(track, at)))
                    .ok();
            }
        })
        .child(icon_inherit(Glyph::Cross, 12.));

    div()
        .flex_none()
        .w(px(ACTIONS_W))
        .flex()
        .items_center()
        .justify_end()
        .gap(px(theme::GAP_XS))
        .child(heart)
        .child(more)
        .child(remove)
}

#[cfg(test)]
mod tests {
    use super::{TWO_LINE_MIN, subtitle_parts};
    use crate::entities::ScopeKind;
    use crate::shell::Density;

    #[test]
    fn the_two_line_threshold_separates_the_two_densities() {
        assert!(
            Density::Comfortable.row_height() >= TWO_LINE_MIN,
            "舒适档（{}px）应当画两行（阈值 {TWO_LINE_MIN}px）",
            Density::Comfortable.row_height()
        );
        assert!(
            Density::Compact.row_height() < TWO_LINE_MIN,
            "紧凑档（{}px）应当只有一行（阈值 {TWO_LINE_MIN}px）",
            Density::Compact.row_height()
        );
    }

    #[test]
    fn subtitle_drops_the_segment_already_shown_by_the_hero() {
        assert_eq!(subtitle_parts(ScopeKind::Artist, true, true), (false, true));
        assert_eq!(subtitle_parts(ScopeKind::Album, true, true), (true, false));
        assert_eq!(subtitle_parts(ScopeKind::All, true, true), (true, true));
    }

    #[test]
    fn subtitle_parts_never_hide_the_only_meaningful_segment() {
        // 副标题整块为空是允许的：不给点进空作用域的链接
        assert_eq!(
            subtitle_parts(ScopeKind::Artist, true, false),
            (false, false)
        );
        assert_eq!(
            subtitle_parts(ScopeKind::Album, false, true),
            (false, false)
        );
    }
}
