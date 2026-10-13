//! 右侧面板的「队列」页签：显示真实播放顺序（`Playlist::order`），随机模式下与曲库顺序完全不同。
//! 不参与搜索、不做增删 —— 队列是由播放模式决定的产物。

use lyre_core::{PlayMode, Track, TrackId, format_duration};

use crate::common::thousands;
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon, icon_inherit};
use crate::platform::*;
use crate::theme;

/// 已播只留 5 首作上下文，不属于队列本身。
/// 这是显示上限，不是滚动锚点 —— 锚点是 `QueueRows::current_row`。
const PLAYED_CONTEXT: usize = 5;

/// 行高要容得下 14px 标题行 + 12px 副标题行，合计约 51。
const ROW_H: f32 = theme::ROW_H_COMFORTABLE;

const INDEX_W: f32 = 40.0;
const DURATION_W: f32 = 48.0;

const DIVIDER_H: f32 = 28.0;

pub struct QueuePanel {
    library: Entity<LibraryEntity>,
    _sub: Subscription,
    scroll: UniformListScrollHandle,
    // `observe(library)` 在任何曲库变化时都会触发，所以记住上次定位的曲目，
    // 只在正在播放的那首真的换了时才滚动，否则用户翻看队列时会被莫名拽回去。
    last_anchored: Option<TrackId>,
}

impl QueuePanel {
    pub fn new(library: Entity<LibraryEntity>, cx: &mut Context<Self>) -> Self {
        // 换曲和切模式都改变队列，所以 observe 整个曲库实体。
        let sub = cx.observe(&library, |this, library, cx| {
            this.anchor(&library, cx);
            cx.notify();
        });
        Self {
            library,
            _sub: sub,
            scroll: UniformListScrollHandle::default(),
            last_anchored: None,
        }
    }

    /// 换曲后把当前这首滚到可视区顶部附近。
    /// `Nearest` 而不是 `Center` / `Top`：已在可视区时什么都不做，不打断用户自己翻看。
    fn anchor(&mut self, library: &Entity<LibraryEntity>, cx: &mut Context<Self>) {
        let current = library.read(cx).now_playing();
        if current == self.last_anchored {
            return;
        }
        self.last_anchored = current;
        if current.is_none() {
            return;
        }

        // 锚点是正在播的那一行本身，不是 `PLAYED_CONTEXT`：已播那段实际是 0~4 首，
        // 分段标签还要再占一行。行号只能由 `build_entries` 给出。
        let read = library.read(cx);
        let rows = build_entries(
            read.just_played(PLAYED_CONTEXT),
            read.upcoming(usize::MAX),
            current,
        );
        if let Some(row) = rows.current_row {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
    }
}

impl Render for QueuePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let library = self.library.read(cx);
        let playing = library.now_playing();
        let mode = library.mode();
        let played = library.just_played(PLAYED_CONTEXT);
        // 待播段不截断：整条队列都要能滚到，列表本身是虚拟化的，多几行不增加渲染成本。
        let upcoming = library.upcoming(usize::MAX);
        // 「共 N 首」说的是队列，不是曲库 —— 有队列时两者不相等（列着 4 首却写 131 首）。
        let total = library.queue_len();

        let header = div()
            .flex_none()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(theme::GAP_SM))
            .px(px(theme::GAP_MD))
            .h(px(44.))
            .border_b_1()
            .border_color(theme::border_soft())
            .child(
                div()
                    .flex_none()
                    .text_size(px(theme::FONT_SUBTITLE))
                    .font_weight(theme::WEIGHT_SUBTITLE)
                    .text_color(theme::text())
                    .child("播放队列"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(theme::text_muted())
                    .child(thousands(total)),
            )
            .child(mode_badge(mode));

        let summary = div()
            .flex_none()
            .w_full()
            .min_w_0()
            .truncate()
            .px(px(theme::GAP_MD))
            .py(px(theme::GAP_SM))
            .text_size(px(theme::FONT_CAPTION))
            .text_color(theme::text_muted())
            .child(if total == 0 {
                "曲库为空".to_owned()
            } else if playing.is_none() {
                format!("共 {} 首 · 尚未开始播放", thousands(total))
            } else {
                format!(
                    "共 {} 首 · 接下来 {} 首",
                    thousands(total),
                    thousands(upcoming.len().saturating_sub(1))
                )
            });

        let rows = build_entries(played, upcoming, playing);

        let row_count = rows.entries().len();
        let library_entity = self.library.clone();

        let body: AnyElement = if row_count == 0 {
            empty_queue(total)
        } else {
            uniform_list("queue-list", row_count, move |range, _window, cx| {
                let visible: Vec<QueueEntry> = rows.entries()[range.clone()].to_vec();
                let library = &library_entity;
                visible
                    .into_iter()
                    .map(|entry| match entry {
                        QueueEntry::Track {
                            track,
                            kind,
                            number,
                        } => queue_row(number, &track, kind, library, cx).into_any_element(),
                        QueueEntry::Divider(label) => section_divider(label).into_any_element(),
                    })
                    .collect::<Vec<AnyElement>>()
            })
            .size_full()
            .track_scroll(&self.scroll)
            .into_any_element()
        };

        div()
            .flex_none()
            .w(px(theme::RIGHT_PANEL_W))
            // 只写 `w()` 压不住内容：flex 默认 `min-width: auto`，长歌名会把面板顶宽。`min_w` + `max_w` 钉死。
            .min_w(px(theme::RIGHT_PANEL_W))
            .max_w(px(theme::RIGHT_PANEL_W))
            .overflow_hidden()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .child(header)
            .child(summary)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .relative()
                    .child(body)
                    .child(crate::common::scroll::scrollbar(
                        &self.scroll,
                        "queue-list-scrollbar",
                    )),
            )
    }
}

/// 队列列表里的一项：曲目或分段标签。
#[derive(Clone)]
enum QueueEntry {
    /// 一行曲目。`number` 是显示序号，按曲目数而不是行数 —— 分段标签占一行但不占号，
    /// 否则编号会在标签处跳一格（`5 → 7`），看起来像少了一首。
    Track {
        track: Track,
        kind: RowKind,
        number: usize,
    },
    /// 带文字的刻度行（「接下来」/「待播放」）。
    Divider(&'static str),
}

/// 队列的行模型：行本身，加上正在播的那一行的下标。滚动锚点只能从这里拿。
/// 「已播那段有几行」不等于 `PLAYED_CONTEXT`：刚开播时是空的、只往回取 5 首、分段标签还占一行。
struct QueueRows {
    entries: Vec<QueueEntry>,
    /// 正在播的那一行的下标。没有正在播的曲目、或它不在待播区里时为 `None`。
    current_row: Option<usize>,
}

impl QueueRows {
    fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }
}

/// 把「已播 / 待播」两段拼成一条连续队列，中间插分段标签。
/// `upcoming` 第一首只有与 `now_playing` 相同才算 [`RowKind::Current`]。
/// 标签只在两段都非空时插入；序号按曲目连续、标签不占号。
fn build_entries(played: Vec<Track>, upcoming: Vec<Track>, current: Option<TrackId>) -> QueueRows {
    let played_count = played.len();
    let mut rows: Vec<QueueEntry> = Vec::with_capacity(played_count + upcoming.len() + 1);
    let mut number = 1;

    for track in played {
        rows.push(QueueEntry::Track {
            track,
            kind: RowKind::Played,
            number,
        });
        number += 1;
    }

    let mut rest = upcoming.into_iter();
    let mut current_row = None;
    if let Some(track) = rest.next() {
        let kind = if current == Some(track.id) {
            RowKind::Current
        } else {
            RowKind::Upcoming
        };
        // 这里数出来的才是真行号：上面已播那段在开头可能是空的。
        if kind == RowKind::Current {
            current_row = Some(rows.len());
        }
        rows.push(QueueEntry::Track {
            track,
            kind,
            number,
        });
        number += 1;
    }
    for track in rest {
        rows.push(QueueEntry::Track {
            track,
            kind: RowKind::Upcoming,
            number,
        });
        number += 1;
    }

    if played_count > 0 && rows.len() > played_count {
        let label = if current.is_some() {
            "接下来"
        } else {
            "待播放"
        };
        rows.insert(played_count, QueueEntry::Divider(label));
        // 标签插在已播段之后，把当前行连同它后面的一切都推下去一行 ——
        // 有当前行时它必然正好是已播段后面那第一行。
        if let Some(row) = current_row.as_mut() {
            *row += 1;
        }
    }

    QueueRows {
        entries: rows,
        current_row,
    }
}

fn section_divider(label: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .h(px(DIVIDER_H))
        .flex()
        .items_center()
        .gap(px(theme::GAP_SM))
        .px(px(theme::GAP_MD))
        .child(div().flex_1().h(px(1.)).bg(theme::border_soft()))
        .child(
            div()
                .flex_none()
                .child(crate::common::section::section_heading(label)),
        )
        .child(div().flex_1().h(px(1.)).bg(theme::border_soft()))
}

/// 空队列状态。「曲库为空」和「有库但没开始播」的文案必须分开，否则会被读成导入失败。
fn empty_queue(total: usize) -> AnyElement {
    let (hint, detail) = if total == 0 {
        ("队列是空的", "先导入音乐，队列才会有内容")
    } else {
        ("尚未开始播放", "在上面挑一首开始，这里就会跟上")
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::GAP_SM))
        .px(px(theme::GAP_LG))
        .child(icon(Glyph::Queue, 32., theme::text_faint()))
        .child(
            div()
                .text_size(px(theme::FONT_BODY))
                .text_color(theme::text_muted())
                .child(hint),
        )
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_faint())
                .child(detail),
        )
        .into_any_element()
}

/// 一行在队列里的身份。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKind {
    Played,
    /// 正在播 / 按下播放会从这里开始。
    Current,
    Upcoming,
}

/// 模式徽标：随机模式下队列顺序与曲库完全不同，得在旁边说明这不是排序错了。
fn mode_badge(mode: PlayMode) -> impl IntoElement {
    let (label, glyph, active) = match mode {
        PlayMode::Sequential => ("顺序", Glyph::List, false),
        PlayMode::Shuffle => ("随机", Glyph::Shuffle, true),
        PlayMode::RepeatAll => ("列表循环", Glyph::RepeatAll, true),
        PlayMode::RepeatOne => ("单曲循环", Glyph::RepeatOne, true),
    };

    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(theme::GAP_XS))
        .px(px(theme::GAP_SM))
        .py(px(2.))
        .rounded(px(theme::RADIUS_PILL))
        .bg(if active {
            theme::control()
        } else {
            theme::elevated()
        })
        .text_color(if active {
            theme::text_on_control()
        } else {
            theme::text_muted()
        })
        .text_size(px(theme::FONT_MICRO))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .child(icon_inherit(glyph, 12.)),
        )
        .child(label)
}

/// 队列的一行。`kind` 决定配色，点一下跳到这一首。
/// `number` 既用于显示也用于元素 id：id 跟着曲目而不是行数，换曲时标签增减才不会让滚动错位。
fn queue_row(
    number: usize,
    track: &Track,
    kind: RowKind,
    library: &Entity<LibraryEntity>,
    _cx: &mut App,
) -> impl IntoElement {
    let track_id = track.id;
    let duration = track
        .duration
        .map_or_else(|| "--:--".to_owned(), format_duration);
    let current = kind == RowKind::Current;
    let played = kind == RowKind::Played;

    let title_color = match kind {
        RowKind::Current => theme::accent_text(),
        RowKind::Played => theme::text_muted(),
        RowKind::Upcoming => theme::text(),
    };
    let meta_color = match kind {
        RowKind::Current => theme::accent_text(),
        RowKind::Played => theme::text_faint(),
        RowKind::Upcoming => theme::text_muted(),
    };

    // 每行独立的 group 名：共用一个的话鼠标进任意一行、所有行都会亮。
    let group: SharedString = format!("queue-row-{number}").into();

    let mut row = div()
        .id(("queue-row", number))
        .group(group.clone())
        .flex_none()
        // `uniform_list` 的行默认按内容定宽：`w_full` 才让时长有右对齐的参照系，`min_w_0` 才允许标题列收缩。
        .w_full()
        .min_w_0()
        .h(px(ROW_H))
        .flex()
        .items_center()
        .px(px(theme::GAP_MD))
        .cursor_pointer()
        .bg(if current {
            theme::accent_soft()
        } else {
            theme::transparent()
        });

    if !current {
        row = row.hover(|style| style.bg(theme::hover()));
    }

    row = row.on_click({
        let library = library.clone();
        move |_, _, cx| {
            library.update(cx, |library, cx| {
                library.activate(track_id, cx);
            });
        }
    });

    row = row
        // 竖条不用 `border`：那会把内容整体往右推，切换当前曲时整列文字横跳。
        .child(
            div()
                .flex_none()
                .w(px(2.))
                .h(px(ROW_H * 0.5))
                .rounded_full()
                .bg(if current {
                    theme::accent()
                } else {
                    theme::transparent()
                }),
        )
        .child(index_cell(
            number,
            current,
            group.clone(),
            track_id,
            library,
        ))
        .child(title_cell(track, title_color, meta_color));

    // 时长列固定宽度而不是 `flex_1`：后者的右边界取决于标题长度，列位置会随内容浮动。
    let row = row.child(
        div()
            .flex_none()
            .w(px(DURATION_W))
            .flex()
            .justify_end()
            .text_size(px(theme::FONT_CAPTION))
            .text_color(if played {
                theme::text_faint()
            } else {
                theme::text_muted()
            })
            .child(duration),
    );

    // 菜单与曲库行 / 搜索结果共用一份，但这里的「播放」只能跳播、不能重设队列 —— 否则随机模式会被洗回曲库顺序。
    let hooks = crate::common::menu::TrackMenuHooks::none().play({
        let library = library.clone();
        move |id, cx| {
            library.update(cx, |library, cx| {
                library.activate(id, cx);
            });
        }
    });
    row.context_menu(crate::common::menu::track_context_menu(
        library.clone(),
        track_id,
        hooks,
    ))
}

/// 序号位：平时是序号，悬停时换成 ▶，点一下跳到这一首。
/// 两层重叠在同一格、用 `group_hover` 切透明度：每行维护 `hovered` 布尔的话，鼠标滑过每行都会 `notify`。
fn index_cell(
    number: usize,
    current: bool,
    group: SharedString,
    track_id: lyre_core::TrackId,
    library: &Entity<LibraryEntity>,
) -> impl IntoElement {
    if current {
        return div()
            .flex_none()
            .w(px(INDEX_W))
            .flex()
            .items_center()
            .justify_end()
            .pr(px(theme::GAP_SM))
            .child(icon(Glyph::Music, 12., theme::accent_text()))
            .into_any_element();
    }

    div()
        .id(("queue-index", number))
        .flex_none()
        .relative()
        .w(px(INDEX_W))
        .flex()
        .items_center()
        .justify_end()
        .pr(px(theme::GAP_SM))
        .cursor_pointer()
        .on_click({
            let library = library.clone();
            move |_, _, cx| {
                // 必须阻断冒泡：整行上已经有一个 `activate` 回调，不阻断的话一次点击会重播两遍。
                cx.stop_propagation();
                library.update(cx, |library, cx| {
                    library.activate(track_id, cx);
                });
            }
        })
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_faint())
                .group_hover(group.clone(), |style| style.opacity(0.0))
                .child(format!("{number}")),
        )
        .child(
            div()
                .absolute()
                .right(px(theme::GAP_SM))
                .top(relative(0.5))
                .mt(px(-6.))
                .size(px(12.))
                .flex()
                .items_center()
                .justify_center()
                .opacity(0.0)
                .group_hover(group, |style| style.opacity(1.0))
                .child(icon(Glyph::Play, 10., theme::text())),
        )
        .into_any_element()
}

/// 标题格：第一行歌名，第二行「艺术家 · 专辑」；`flex_1` + `min_w_0` 让时长列位置稳定。
fn title_cell(track: &Track, title_color: Rgba, meta_color: Rgba) -> impl IntoElement {
    let subtitle = track.subtitle();

    let title_line = div()
        .flex()
        .items_center()
        .gap(px(theme::GAP_SM))
        .w_full()
        .min_w_0()
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(theme::FONT_TRACK))
                .text_color(title_color)
                .child(track.title.clone()),
        );

    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(1.))
        .child(title_line)
        .when(!subtitle.is_empty(), |cell| {
            cell.child(
                div()
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(meta_color)
                    .truncate()
                    .child(subtitle),
            )
        })
}

#[cfg(test)]
mod tests {
    // 不用 glob 导入：`platform::*` 里的 gpui `test` 宏会遮蔽内置 `#[test]`，导致无限递归。
    use super::{PLAYED_CONTEXT, QueueEntry, QueueRows, RowKind, build_entries};
    use lyre_core::Track;
    use std::path::Path;

    fn track(name: &str) -> Track {
        let mut t = Track::fallback(Path::new(&format!("/music/{name}.mp3")));
        t.title = name.to_owned();
        t
    }

    fn titles(rows: &QueueRows) -> Vec<String> {
        rows.entries()
            .iter()
            .filter_map(|row| match row {
                QueueEntry::Track { track, .. } => Some(track.title.clone()),
                QueueEntry::Divider(_) => None,
            })
            .collect()
    }

    fn kinds(rows: &QueueRows) -> Vec<RowKind> {
        rows.entries()
            .iter()
            .filter_map(|row| match row {
                QueueEntry::Track { kind, .. } => Some(*kind),
                QueueEntry::Divider(_) => None,
            })
            .collect()
    }

    fn numbers(rows: &QueueRows) -> Vec<usize> {
        rows.entries()
            .iter()
            .filter_map(|row| match row {
                QueueEntry::Track { number, .. } => Some(*number),
                QueueEntry::Divider(_) => None,
            })
            .collect()
    }

    fn divider_at(rows: &QueueRows) -> Option<usize> {
        rows.entries()
            .iter()
            .position(|row| matches!(row, QueueEntry::Divider(_)))
    }

    fn row_marked_current(rows: &QueueRows) -> Option<usize> {
        rows.entries().iter().position(|row| {
            matches!(
                row,
                QueueEntry::Track {
                    kind: RowKind::Current,
                    ..
                }
            )
        })
    }

    #[test]
    fn first_upcoming_is_the_current_track() {
        // `upcoming()` 从 cursor 起取，第一首就是光标所在那首；错了整条分界线会偏一首。
        let rows = build_entries(
            vec![track("a"), track("b")],
            vec![track("c"), track("d"), track("e")],
            Some(track("c").id),
        );

        assert_eq!(titles(&rows), ["a", "b", "c", "d", "e"]);
        assert_eq!(
            kinds(&rows),
            [
                RowKind::Played,
                RowKind::Played,
                RowKind::Current,
                RowKind::Upcoming,
                RowKind::Upcoming,
            ]
        );
    }

    #[test]
    fn the_cursor_row_is_not_current_until_it_actually_plays() {
        // 刚导入时光标指向第一首但并没在播，没有 `now_playing` 就只能算待播。
        let rows = build_entries(Vec::new(), vec![track("a"), track("b")], None);

        assert_eq!(kinds(&rows), [RowKind::Upcoming, RowKind::Upcoming]);
        assert_eq!(titles(&rows), ["a", "b"]);
    }

    #[test]
    fn a_different_track_playing_leaves_the_cursor_row_upcoming() {
        // 正在播的那首不在待播区里时，光标那一行同样不该点亮。
        let rows = build_entries(
            vec![track("a")],
            vec![track("b"), track("c")],
            Some(track("a").id),
        );

        assert_eq!(
            kinds(&rows),
            [RowKind::Played, RowKind::Upcoming, RowKind::Upcoming]
        );
    }

    #[test]
    fn divider_sits_between_played_and_current() {
        let rows = build_entries(
            vec![track("a")],
            vec![track("b"), track("c")],
            Some(track("b").id),
        );

        assert_eq!(divider_at(&rows), Some(1));
        assert_eq!(rows.entries().len(), 4, "1 已播 + 1 标签 + 2 曲目");
    }

    #[test]
    fn no_divider_when_nothing_has_played_yet() {
        let rows = build_entries(
            Vec::new(),
            vec![track("a"), track("b")],
            Some(track("a").id),
        );

        assert_eq!(divider_at(&rows), None);
        assert_eq!(titles(&rows), ["a", "b"]);
    }

    #[test]
    fn no_divider_when_there_is_nothing_ahead() {
        let rows = build_entries(
            vec![track("a"), track("b")],
            Vec::new(),
            Some(track("b").id),
        );

        assert_eq!(divider_at(&rows), None);
        assert_eq!(titles(&rows), ["a", "b"]);
    }

    #[test]
    fn divider_label_follows_playback_state() {
        // 未在播放时，下面的是「从这里开始播」而不是「接下来会播的」。
        let playing = build_entries(vec![track("a")], vec![track("b")], Some(track("b").id));
        let stopped = build_entries(vec![track("a")], vec![track("b")], None);

        let label = |rows: &QueueRows| match rows.entries().iter().find_map(|r| match r {
            QueueEntry::Divider(l) => Some(*l),
            // 显式列出变体而不是 `_`：给 `QueueEntry` 加新变体时这里会编译失败。
            QueueEntry::Track { .. } => None,
        }) {
            Some(l) => l,
            None => panic!("这一段应当有分段标签"),
        };

        assert_eq!(label(&playing), "接下来");
        assert_eq!(label(&stopped), "待播放");
    }

    #[test]
    fn empty_queue_stays_empty() {
        let rows = build_entries(Vec::new(), Vec::new(), None);
        assert!(rows.entries().is_empty());
        assert_eq!(rows.current_row, None);
    }

    #[test]
    fn track_numbers_stay_contiguous_across_the_divider() {
        let rows = build_entries(
            vec![track("a"), track("b"), track("c")],
            vec![track("d"), track("e")],
            Some(track("d").id),
        );

        assert_eq!(divider_at(&rows), Some(3), "先确认标签确实插在中间");
        assert_eq!(numbers(&rows), [1, 2, 3, 4, 5], "序号必须连续");
    }

    #[test]
    fn track_numbers_start_at_one_without_a_divider() {
        // 无标签时同样从 1 递增：防的是「按行号算」的实现。
        let rows = build_entries(
            Vec::new(),
            vec![track("a"), track("b")],
            Some(track("a").id),
        );
        assert_eq!(divider_at(&rows), None);
        assert_eq!(numbers(&rows), [1, 2]);
    }

    /// 滚动锚点是正在播的那一行本身，已播那段多长都不影响这个性质。
    /// 这一条覆盖 0~5 档：把 `PLAYED_CONTEXT` 当锚点时，短的几档全会偏。
    #[test]
    fn the_anchor_is_the_current_row_however_long_the_played_block_is() {
        for played in 0..=PLAYED_CONTEXT {
            let played_tracks: Vec<Track> = (0..played).map(|i| track(&format!("p{i}"))).collect();
            let head = track("cur");
            let upcoming = vec![head.clone(), track("n1"), track("n2")];

            let rows = build_entries(played_tracks, upcoming, Some(head.id));
            assert_eq!(
                rows.current_row,
                row_marked_current(&rows),
                "已播 {played} 首时，锚点必须正好落在标着「当前」的那一行"
            );
        }
    }

    /// 刚开播：已播区是空的，当前就是第 0 行 —— 锚点不能取 `PLAYED_CONTEXT` 那个常数。
    #[test]
    fn a_queue_that_just_started_anchors_on_row_zero() {
        let head = track("cur");
        let rows = build_entries(Vec::new(), vec![head.clone(), track("n1")], Some(head.id));

        assert_eq!(divider_at(&rows), None, "没有已播就不该有分段标签");
        assert_eq!(rows.current_row, Some(0));
    }

    /// 分段标签也占一行，锚点必须把它算进去 —— 少算一行就正好停在前一首上。
    #[test]
    fn the_divider_row_pushes_the_anchor_down_by_one() {
        let head = track("cur");
        let rows = build_entries(
            vec![track("p0")],
            vec![head.clone(), track("n1")],
            Some(head.id),
        );

        assert_eq!(divider_at(&rows), Some(1));
        assert_eq!(
            rows.current_row,
            Some(2),
            "已播 1 行 + 标签 1 行，当前在第 2 行"
        );
    }

    /// 没在播时下面那段是「待播放」，第一行不算「当前」—— 没有锚点，别乱滚。
    #[test]
    fn there_is_no_anchor_when_nothing_is_playing() {
        let rows = build_entries(vec![track("p0")], vec![track("a"), track("b")], None);

        assert_eq!(row_marked_current(&rows), None);
        assert_eq!(rows.current_row, None);
    }

    /// 正在播的那首不在待播区里时同样没有锚点：此时一条高亮都没有，滚到哪里都是错的。
    #[test]
    fn there_is_no_anchor_when_the_playing_track_is_off_the_upcoming_block() {
        let rows = build_entries(
            vec![track("p0")],
            vec![track("a"), track("b")],
            Some(track("p0").id),
        );

        assert_eq!(rows.current_row, None);
    }
}
