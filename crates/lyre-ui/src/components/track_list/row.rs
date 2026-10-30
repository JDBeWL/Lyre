//! 行的四态、一行的骨架，以及列表的列头。

use lyre_core::{Track, TrackId, format_duration};

use crate::cover::CoverCache;
use crate::entities::{LibraryEntity, ScopeKind};
use crate::icon::{Glyph, icon};
use crate::platform::*;
use crate::theme;

use super::{
    ACTIONS_W, ALBUM_W, CHECK_W, DURATION_W, HANDLE_W, INDEX_W, RowEnv, TrackList, TrackListEvent,
    cells, drag,
};

/// 专辑列要不要画：断点允许，且专辑详情里永远不画（每行都是同一张专辑）。
pub(super) fn show_album_column(breakpoint_allows: bool, kind: ScopeKind) -> bool {
    breakpoint_allows && kind != ScopeKind::Album
}

/// 列头。`trailing` 是行操作列那一格里的附加控件（详情页 / 搜索页的「多选」开关）。
pub(super) fn column_header(env: &RowEnv, trailing: Option<AnyElement>) -> impl IntoElement {
    let mut row = div()
        .flex_none()
        .h(px(theme::COLUMN_HEADER_H))
        .flex()
        .items_center()
        .pl(px(theme::ROW_PAD_LEFT))
        .pr(px(theme::GAP_MD))
        .border_b_1()
        .border_color(theme::border_soft())
        .text_size(px(theme::FONT_CAPTION))
        .text_color(theme::text_muted())
        // 勾选列与手柄列没有表头文字，但要占同样的宽：否则切换多选会在每行前面挖出一道错位。
        .when(env.selectable, |row| {
            row.child(div().flex_none().w(px(CHECK_W)))
        })
        .when(env.reorderable, |row| {
            row.child(div().flex_none().w(px(HANDLE_W)))
        })
        .child(div().flex_none().w(px(INDEX_W)))
        .child(div().flex_1().min_w_0().child("标题"));

    if env.show_album {
        row = row.child(div().flex_none().w(px(ALBUM_W)).child("专辑"));
    }
    row.child(
        div()
            .flex_none()
            .w(px(DURATION_W))
            .flex()
            .justify_end()
            .pr(px(theme::GAP_MD))
            .child("时长"),
    )
    .child(
        div()
            .flex_none()
            .w(px(ACTIONS_W))
            .flex()
            .items_center()
            .justify_end()
            .children(trailing),
    )
}

/// 行状态，四个态互斥。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RowState {
    Idle,
    /// 光标所在行（键盘操作的目标）
    Focused,
    /// 正在播放
    Playing,
    /// 既在播放又是光标行
    PlayingFocused,
}

impl RowState {
    pub(super) fn of(id: TrackId, playing: Option<TrackId>, focus: Option<TrackId>) -> Self {
        match (playing == Some(id), focus == Some(id)) {
            (true, true) => Self::PlayingFocused,
            (true, false) => Self::Playing,
            (false, true) => Self::Focused,
            (false, false) => Self::Idle,
        }
    }

    pub(super) fn is_playing(self) -> bool {
        matches!(self, Self::Playing | Self::PlayingFocused)
    }

    fn is_focused(self) -> bool {
        matches!(self, Self::Focused | Self::PlayingFocused)
    }

    fn background(self) -> Rgba {
        match self {
            Self::Playing | Self::PlayingFocused => theme::accent_soft(),
            Self::Focused => theme::cursor(),
            Self::Idle => theme::transparent(),
        }
    }

    /// 画不画行首那道竖条（= 键盘光标在哪）：两个播放态的底色与字色相同，只能靠它分开。
    /// 竖条是绝对定位，不做 flex 子元素或 `border` —— 那两者参与布局，会把整表的列推歪。
    fn shows_cursor_bar(self) -> bool {
        self.is_focused()
    }

    pub(super) fn title_color(self) -> Rgba {
        if self.is_playing() {
            theme::accent_text()
        } else {
            theme::text()
        }
    }

    pub(super) fn title_weight(self) -> FontWeight {
        if self.is_playing() {
            theme::WEIGHT_SUBTITLE
        } else {
            theme::WEIGHT_TRACK
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn row(
    index: usize,
    track: &Track,
    state: RowState,
    picked: bool,
    env: RowEnv,
    library: &Entity<LibraryEntity>,
    list: &WeakEntity<TrackList>,
    covers: &Entity<CoverCache>,
    cx: &mut App,
) -> AnyElement {
    let track_id = track.id;
    // 行高只从 `env.row_height` 来：`uniform_list` 按它算每一项的位置，这里另算一份就会错位
    let row_height = env.row_height;
    let group: SharedString = format!("track-row-{index}").into();
    let liked = library.read(cx).is_liked(track_id);

    let mut element = div()
        .id(("track-row", index))
        .group(group.clone())
        // 落点区是绝对定位的，要有这个才钉在本行而不是钉在列表上
        .relative()
        .h(px(row_height))
        .w_full()
        // `min_w_0` 与 `w_full` 配套：否则行收缩不到比内容更窄，长标题会把专辑列和时长列顶歪
        .min_w_0()
        .flex()
        .items_center()
        .pl(px(theme::ROW_PAD_LEFT))
        .pr(px(theme::GAP_MD))
        .bg(state.background());

    // 悬停底色只给空闲行：`hover` 比 `accent_soft` 与 `cursor` 都暗，叠上去会把播放行和光标行压暗
    if matches!(state, RowState::Idle) {
        element = element.hover(|style| style.bg(theme::hover()));
    }

    // `left(0)` 相对行的 padding box，绝对定位不挤占任何一列
    if state.shows_cursor_bar() {
        element = element.child(
            div()
                .absolute()
                .left(px(0.))
                .top(px(row_height * 0.25))
                .w(px(theme::ROW_CURSOR_BAR_W))
                .h(px(row_height * 0.5))
                .rounded_full()
                .bg(theme::text()),
        );
    }

    // 行首两列都定宽：切换多选或进出歌单不该让整表的列跳一下
    if env.selectable {
        element = element.child(cells::check_cell(index, track_id, picked, list.clone()));
    }
    if env.reorderable {
        // 拖拽影像要的是标题：只有真能拖的时候才付一次 clone
        element = element.child(drag::handle_cell(index, track.title.clone(), group.clone()));
    }

    let index_cell: AnyElement = if state.is_playing() {
        div()
            .flex_none()
            .w(px(INDEX_W))
            .flex()
            .justify_end()
            .pr(px(theme::ROW_INDEX_GAP))
            .child(icon(Glyph::Music, 12., theme::accent_text()))
            .into_any_element()
    } else {
        // 序号与悬停播放键重叠在同一格，用 `group_hover` 切换两层透明度。
        // 不用父级 `hover` + 每行的 `hovered` 布尔：滑过一行就 notify 一次，滚动时每秒几十次重绘
        let number_color = if state.is_focused() {
            theme::text_muted()
        } else {
            theme::text_faint()
        };

        div()
            .id(("track-index", index))
            .flex_none()
            .relative()
            .w(px(INDEX_W))
            .flex()
            .items_center()
            .justify_end()
            .pr(px(theme::ROW_INDEX_GAP))
            .cursor_pointer()
            // 点序号 = 播放这一首，与点标题同走 `activate`
            .on_click({
                let library = library.clone();
                move |_, _, cx| {
                    library.update(cx, |library, cx| {
                        library.activate_in_view(track_id, cx);
                    });
                }
            })
            .child(
                div()
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(number_color)
                    .group_hover(group.clone(), |style| style.opacity(0.0))
                    .child(format!("{}", index + 1)),
            )
            .child(
                div()
                    .absolute()
                    .right(px(theme::ROW_INDEX_GAP))
                    .top(relative(0.5))
                    .mt(px(-6.))
                    .size(px(12.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(0.0)
                    .group_hover(group.clone(), |style| style.opacity(1.0))
                    .child(icon(Glyph::Play, 10., theme::text())),
            )
            .into_any_element()
    };

    element = element.child(index_cell).child(cells::title_cell(
        index, track, state, env, library, list, covers, track_id, cx,
    ));

    if env.show_album {
        element = element.child(cells::album_cell(
            index,
            track.album.as_deref(),
            library.clone(),
            list.clone(),
        ));
    }

    let hooks = crate::common::menu::TrackMenuHooks::none()
        .before_leave({
            let list = list.clone();
            move |cx| {
                // 先报落点再收窄作用域，顺序反了会记成「进入之后」
                list.update(cx, |_, cx| cx.emit(TrackListEvent::RecordFromTrack))
                    .ok();
            }
        })
        .after_remove({
            let list = list.clone();
            move |track, at, cx| {
                list.update(cx, |_, cx| cx.emit(TrackListEvent::Removed(track, at)))
                    .ok();
            }
        });
    let menu = || crate::common::menu::track_context_menu(library.clone(), track_id, hooks.clone());

    element = element
        .child(
            div()
                .flex_none()
                .w(px(DURATION_W))
                .flex()
                .justify_end()
                .pr(px(theme::GAP_MD))
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_muted())
                .child(
                    track
                        .duration
                        .map_or_else(|| "--:--".to_owned(), format_duration),
                ),
        )
        .child(cells::actions_cell(
            index,
            state,
            group,
            track_id,
            liked,
            library.clone(),
            list.clone(),
            menu(),
        ));

    // 落点高亮用 `drag_over` 而不是视图状态：拖到行外松手不会留下残留高亮
    if env.reorderable {
        element = element
            .child(drag::drop_zone(
                index,
                drag::DropEdge::Above,
                row_height,
                library.clone(),
            ))
            .child(drag::drop_zone(
                index,
                drag::DropEdge::Below,
                row_height,
                library.clone(),
            ));
    }

    // 引擎解不了的格式照常列出，只是压暗并给出原因（判据见 `lyre_core::UNSUPPORTED_EXTENSIONS`）
    if let Some(reason) = track.unplayable_reason() {
        element = element
            .opacity(theme::UNPLAYABLE_OPACITY)
            .tooltip(crate::common::tooltip::builder(reason));
    }

    // `context_menu` 返回 `ContextMenu<Div>` 而不是 `Div`，只能放在链式末尾。
    element.context_menu(menu()).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{RowState, show_album_column};
    use crate::entities::ScopeKind;
    use lyre_core::TrackId;

    #[test]
    fn album_column_vanishes_inside_an_album_detail() {
        assert!(show_album_column(true, ScopeKind::All));
        assert!(show_album_column(true, ScopeKind::Artist));
        assert!(!show_album_column(true, ScopeKind::Album));
        assert!(!show_album_column(false, ScopeKind::All));
    }

    #[test]
    fn row_state_separates_cursor_from_playback() {
        use RowState::{Focused, Idle, Playing, PlayingFocused};
        let a = TrackId(1);
        let b = TrackId(2);

        assert_eq!(RowState::of(a, Some(a), Some(a)), PlayingFocused);
        assert_eq!(RowState::of(a, Some(a), Some(b)), Playing);
        assert_eq!(RowState::of(a, Some(b), Some(a)), Focused);
        assert_eq!(RowState::of(a, None, None), Idle);
    }

    #[test]
    fn playing_rows_are_visually_distinct_from_focused_rows() {
        assert!(RowState::Playing.is_playing());
        assert!(!RowState::Focused.is_playing());
        assert!(RowState::Focused.is_focused());
        assert!(!RowState::Playing.is_focused());
        assert_ne!(
            RowState::Playing.title_color(),
            RowState::Focused.title_color()
        );
    }

    #[test]
    fn playing_focused_row_shows_the_cursor_bar() {
        assert_eq!(
            RowState::Playing.background(),
            RowState::PlayingFocused.background()
        );
        assert_eq!(
            RowState::Playing.title_color(),
            RowState::PlayingFocused.title_color()
        );
        assert_eq!(
            RowState::Playing.title_weight(),
            RowState::PlayingFocused.title_weight()
        );

        assert!(!RowState::Playing.shows_cursor_bar());
        assert!(RowState::PlayingFocused.shows_cursor_bar());
        assert!(RowState::Focused.shows_cursor_bar());
        assert!(!RowState::Idle.shows_cursor_bar());
    }

    #[test]
    fn cursor_row_is_not_the_hover_color() {
        assert_ne!(RowState::Focused.background(), crate::theme::hover());
        assert_ne!(RowState::Focused.background(), RowState::Idle.background());
    }

    #[test]
    fn playing_row_is_bold_so_color_is_not_the_only_channel() {
        assert_ne!(
            RowState::Playing.title_weight(),
            RowState::Idle.title_weight()
        );
        assert_ne!(
            RowState::Playing.title_weight(),
            RowState::Focused.title_weight()
        );
    }
}
