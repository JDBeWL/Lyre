//! 曲库列表，用 `uniform_list` 虚拟化。

mod batch;
mod cells;
mod drag;
mod empty;
mod row;
mod selection;
mod toolbar;

use std::collections::HashSet;
use std::rc::Rc;

use lyre_core::{Track, TrackId};

use crate::cover::CoverCache;
use crate::entities::{LibraryEntity, LibraryEvent, Scope, ScopeKind};
use crate::platform::*;
use crate::shell::ShellEntity;

/// 要能放下两位数和 ♪ / ▶ 而不跳动。
const INDEX_W: f32 = 40.0;
/// 拖拽手柄列宽：只在歌单作用域出现。
const HANDLE_W: f32 = 26.0;
/// 勾选列宽：只在多选模式出现。
const CHECK_W: f32 = 32.0;
const CHECK_BOX: f32 = 18.0;
/// 标题行首的封面缩略图边长（舒适密度）。
const THUMB: f32 = 40.0;
/// 紧凑密度下的封面边长：40px 行高塞不下 40px 封面 + 文字。
const THUMB_COMPACT: f32 = 32.0;
/// 专辑列宽，窄窗口下整列隐藏。
const ALBUM_W: f32 = 220.0;
/// 时长列定宽，避免数字宽度跳动。
const DURATION_W: f32 = 60.0;
/// 三颗 28px 按钮 + 两道 4px 间隙；列头共用同一个常量。
const ACTIONS_W: f32 = 92.0;

/// 列表向上层汇报的事件。用事件而不是回调字段：列表不持有 `RootView`。
#[derive(Clone, Debug, PartialEq)]
pub enum TrackListEvent {
    /// 移除了一首，附原曲目与原行号（撤销要用）
    Removed(Track, usize),
    /// 清空了曲库，附原来的全部曲目
    Cleared(Vec<Track>),
    /// 即将进入专辑 / 艺术家详情。落点由 `RootView` 持有；不发这一句，页头不会出现返回按钮。
    RecordFromTrack,
}

pub struct TrackList {
    library: Entity<LibraryEntity>,
    shell: Entity<ShellEntity>,
    /// 行首缩略图的封面缓存，列表只读、只发请求。
    covers: Entity<CoverCache>,
    scroll: UniformListScrollHandle,
    /// 多选模式开关：开着才画勾选列，点标题也从「播放」变成「选中」。
    selecting: bool,
    /// 已选中的曲目。包一层 `Rc`：交给 `uniform_list` 闭包的拷贝在整库勾选时是 O(n)，每帧一次。
    selected: Rc<HashSet<TrackId>>,
    /// 最近一次点中的行号，Shift 连选从这里起算。
    anchor: Option<usize>,
    /// 上一次渲染时的作用域。换了作用域就清空勾选 —— 勾着的曲目已经不在眼前了。
    scope_seen: Scope,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TrackListEvent> for TrackList {}

impl TrackList {
    pub fn new(
        library: Entity<LibraryEntity>,
        shell: Entity<ShellEntity>,
        covers: Entity<CoverCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&library, |_, _, cx| cx.notify());
        let observe_shell = cx.observe(&shell, |_, _, cx| cx.notify());
        // 封面解码完成时要重绘，否则行首占位图一直挂着
        let observe_covers = cx.observe(&covers, |_, _, cx| cx.notify());

        // 键盘光标移动时滚动跟随。用 `Nearest`：`Center` 每次按方向键都大幅跳动
        let scroll = UniformListScrollHandle::default();
        let follow = {
            let scroll = scroll.clone();
            cx.subscribe(&library, move |_, _, event: &LibraryEvent, _| {
                if let LibraryEvent::FocusMoved(index) = event {
                    scroll.scroll_to_item(*index, ScrollStrategy::Nearest);
                }
            })
        };

        Self {
            scope_seen: library.read(cx).scope().clone(),
            library,
            shell,
            covers,
            scroll,
            selecting: false,
            selected: Rc::new(HashSet::new()),
            anchor: None,
            _subs: vec![observe, observe_shell, observe_covers, follow],
        }
    }
}

impl Render for TrackList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scope_now = self.library.read(cx).scope().clone();
        if scope_now != self.scope_seen {
            self.scope_seen = scope_now;
            Rc::make_mut(&mut self.selected).clear();
            self.anchor = None;
        }

        let (count, total, playing, focus, filtering, sort_key, scanning, scoped, scope, from_list) = {
            let library = self.library.read(cx);
            (
                library.visible_len(),
                library.len(),
                // 用 `now_playing` 而不是队列光标 `current_id`：后者导入完成时
                // 就指向第一首，会把第 1 行误标成播放中
                library.now_playing(),
                // 光标的值一直在（`Enter` / `Delete` 要它），底色只在键盘真的动过之后才画
                library.focus().filter(|_| library.shows_cursor()),
                // 用 `has_query()` 而不是 `is_filtering()`：后者在「只进了一张
                // 专辑、没搜索」时也是 `true`
                library.has_query(),
                library.sort_key(),
                library.scanning(),
                library.scope().is_scoped(),
                library.scope_label(),
                matches!(library.scope(), Scope::Collection(_) | Scope::Favorites),
            )
        };
        let (kind, collection) = {
            let library = self.library.read(cx);
            let scope = library.scope();
            (
                scope.kind(),
                match scope {
                    Scope::Collection(id) => Some(*id),
                    _ => None,
                },
            )
        };
        let reorderable = toolbar::can_reorder(collection.is_some(), filtering, count);

        // 勾选只算看得见的行，否则搜索把行筛掉后「已选 N」把看不见的也算进去
        let chosen = self.chosen(cx);
        let selecting = self.selecting;
        let selected = Rc::clone(&self.selected);

        let env = RowEnv {
            row_height: self.shell.read(cx).row_height(),
            show_album: row::show_album_column(
                self.shell.read(cx).breakpoint().shows_album_column(),
                kind,
            ),
            kind,
            reorderable,
            selectable: selecting,
        };

        // `in_search` 判的是「停在搜索页」而不是「有搜索词」：曲库页用顶栏过滤时仍算曲库页
        let in_search = self.shell.read(cx).nav_section() == crate::shell::NavSection::Search;
        let headed_elsewhere = scoped || in_search;

        let list = Self::weak(cx);
        let header = div()
            .flex_none()
            .flex()
            .flex_col()
            .child(toolbar::toolbar(
                self.library.clone(),
                list.clone(),
                toolbar::ToolbarState {
                    count,
                    total,
                    filtering,
                    sort_key,
                    scanning,
                    scoped,
                    scope,
                    headed_elsewhere,
                    in_search,
                    selecting,
                },
            ))
            .when(selecting, |column| {
                column.child(batch::batch_bar(batch::BatchBar {
                    chosen,
                    rows: count,
                    collection,
                    library: self.library.clone(),
                    list: list.clone(),
                }))
            })
            .child(row::column_header(&env));

        let body: AnyElement = if total == 0 {
            empty::empty_library(self.library.clone()).into_any_element()
        } else if count == 0 {
            let (query, scope) = {
                let library = self.library.read(cx);
                (library.query().to_owned(), library.scope_label())
            };
            if query.trim().is_empty() {
                empty::empty_scope(self.library.clone(), scope, from_list).into_any_element()
            } else {
                empty::no_matches(self.library.clone(), total, &query).into_any_element()
            }
        } else {
            let library = self.library.clone();
            let covers = self.covers.clone();
            let list = Self::weak(cx);
            uniform_list("track-list", count, move |range, _window, cx| {
                // 走可见投影而不是曲库本体，只拷可视区那几首
                let tracks = library.read(cx).visible_tracks(range.clone());
                tracks
                    .into_iter()
                    .enumerate()
                    .map(|(offset, track)| {
                        let index = range.start + offset;
                        let state = row::RowState::of(track.id, playing, focus);
                        let picked = selected.contains(&track.id);
                        row::row(
                            index, &track, state, picked, env, &library, &list, &covers, cx,
                        )
                    })
                    .collect::<Vec<AnyElement>>()
            })
            .size_full()
            .track_scroll(&self.scroll)
            .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(header)
            .child(div().flex_1().min_h_0().relative().child(body).child(
                crate::common::scroll::scrollbar(&self.scroll, "track-list-scrollbar"),
            ))
            .into_any_element()
    }
}

/// 行与列头共用的布局与作用域参数。
#[derive(Clone, Copy)]
struct RowEnv {
    /// 行高，来自密度设置；`uniform_list` 用它算每一项的位置
    row_height: f32,
    /// 画不画专辑列：断点允许、且不在专辑详情里
    show_album: bool,
    /// 作用域种类，决定副标题省哪段、列头第二段写谁
    kind: ScopeKind,
    /// 行首拖拽手柄 + 行内落点区
    reorderable: bool,
    selectable: bool,
}
