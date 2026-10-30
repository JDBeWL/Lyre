//! 曲库列表，用虚拟列表渲染。
//! 曲库页工具条固定、只滚行；专辑 / 艺人详情与搜索结果把页头并进同一个滚动区，
//! 这样整页一起滚，滚动条从内容区顶部贯穿到底部。

mod batch;
mod cells;
mod drag;
mod empty;
mod row;
mod selection;
mod toolbar;

use std::collections::HashSet;
use std::rc::Rc;

use lyre_core::{CollectionId, Track, TrackId};

use crate::cover::CoverCache;
use crate::entities::{LibraryEntity, LibraryEvent, Scope, ScopeKind};
use crate::platform::*;
use crate::shell::{Breakpoint, NavSection, ShellEntity};

use super::SearchPage;
use super::detail::{self, HeroInfo};

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

/// 页头与列表合成一页时的页头标识。换了页头（换艺人 / 进出搜索）整表重置。
#[derive(Clone, PartialEq, Eq)]
enum PageHead {
    /// 搜索结果页：摘要就是 [`SearchPage`] 这一块。
    Search,
    /// 专辑 / 艺人详情页：头图从作用域现算。
    Scope(Scope),
}

pub struct TrackList {
    library: Entity<LibraryEntity>,
    shell: Entity<ShellEntity>,
    /// 行首缩略图的封面缓存，列表只读、只发请求。
    covers: Entity<CoverCache>,
    /// 搜索结果页的摘要，合成页时作为列表项 0 的一部分出现在页头里。
    search_page: Entity<SearchPage>,
    scroll: UniformListScrollHandle,
    /// 页头并入滚动区时用的列表：项 0 是页头，之后才是曲目行。
    page: ListState,
    /// 当前合成页的页头；`None` 表示纯列表模式（工具条固定）。
    page_head: Option<PageHead>,
    /// 合成页当前的行数（不含页头那一项）。
    page_rows: usize,
    /// 页头高度依赖查询、多选开关与断点，变化时让页头重新量。
    page_query: String,
    page_breakpoint: Breakpoint,
    page_selecting: bool,
    /// 多选模式开关：开着才画勾选列，点标题也从「播放」变成「选中」。
    selecting: bool,
    /// 已选中的曲目。包一层 `Rc`：交给列表闭包的拷贝在整库勾选时是 O(n)，每帧一次。
    selected: Rc<HashSet<TrackId>>,
    /// 最近一次点中的行号，Shift 连选从这里起算。
    anchor: Option<usize>,
    /// 上次清理选择时对应的可见投影代次。只在换代时清理，渲染本身不再扫全表。
    selection_generation: u64,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TrackListEvent> for TrackList {}

impl TrackList {
    pub fn new(
        library: Entity<LibraryEntity>,
        shell: Entity<ShellEntity>,
        covers: Entity<CoverCache>,
        search_page: Entity<SearchPage>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&library, |_, _, cx| cx.notify());
        let observe_shell = cx.observe(&shell, |_, _, cx| cx.notify());
        // 封面解码完成时要重绘，否则行首占位图一直挂着
        let observe_covers = cx.observe(&covers, |_, _, cx| cx.notify());

        // 键盘光标移动时滚动跟随。用 `Nearest`：`Center` 每次按方向键都大幅跳动
        let scroll = UniformListScrollHandle::default();
        let follow = cx.subscribe(&library, move |this, _, event: &LibraryEvent, cx| {
            if let LibraryEvent::FocusMoved(index) = event {
                if this.page_head.is_some() {
                    // 合成页的项 0 是页头，行号要顺延一个
                    this.page.scroll_to_reveal_item(index + 1);
                    cx.notify();
                } else {
                    this.scroll.scroll_to_item(*index, ScrollStrategy::Nearest);
                }
            }
        });
        let page_breakpoint = shell.read(cx).breakpoint();

        Self {
            selection_generation: library.read(cx).visible_generation(),
            library,
            shell,
            covers,
            search_page,
            scroll,
            page: ListState::new(0, ListAlignment::Top, px(0.)),
            page_head: None,
            page_rows: 0,
            page_query: String::new(),
            page_breakpoint,
            page_selecting: false,
            selecting: false,
            selected: Rc::new(HashSet::new()),
            anchor: None,
            _subs: vec![observe, observe_shell, observe_covers, follow],
        }
    }
}

impl Render for TrackList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 查询 / 作用域 / 删歌都会让可见投影换代；只有这时才需要清掉看不见的勾选。
        let generation = self.library.read(cx).visible_generation();
        if generation != self.selection_generation {
            self.selection_generation = generation;
            self.prune_selection(cx);
        }

        let (
            count,
            total,
            playing,
            focus,
            filtering,
            sort_key,
            scanning,
            scoped,
            scope,
            from_list,
            loading,
        ) = {
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
                library.loading(),
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

        // 选择已经在投影换代时清理过，这里只取数量；有序 ID 留到批量动作执行时再生成。
        let chosen_count = self.selected.len();
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

        // 页头并入滚动区只发生在两种页面：搜索结果、专辑 / 艺人详情。
        // 曲库页没有页头，工具条固定、只滚行。
        let current_scope = self.library.read(cx).scope().clone();
        let hero: Option<HeroInfo> = if self.shell.read(cx).nav_section() == NavSection::Tracks {
            App::update_entity(cx, &self.library, |library, _| {
                detail::hero_for(library, &current_scope)
            })
        } else {
            None
        };
        let page_head = if in_search && filtering && count > 0 {
            Some(PageHead::Search)
        } else if hero.is_some() {
            Some(PageHead::Scope(current_scope))
        } else {
            None
        };
        let query = self.library.read(cx).query().to_owned();
        let breakpoint = self.shell.read(cx).breakpoint();

        let list_weak = Self::weak(cx);
        let toolbar_state = toolbar::ToolbarState {
            count,
            total,
            filtering,
            sort_key,
            scanning,
            scoped,
            scope: scope.clone(),
            headed_elsewhere,
            in_search,
            selecting,
        };

        if let Some(head) = page_head {
            // 页头是列表项 0，整页共用一个滚动条。
            let overdraw = px(env.row_height * 4.0);
            if self.page_head.as_ref() == Some(&head) {
                if count != self.page_rows {
                    self.page.splice(1..self.page_rows + 1, count);
                    // 新行给统一高度提示：否则滚到没渲染过的区域时滚动条长度会缩水
                    self.page = self
                        .page
                        .clone()
                        .with_uniform_item_height(px(env.row_height));
                    self.page_rows = count;
                }
                if query != self.page_query {
                    if head == PageHead::Search {
                        // 结果摘要的高度随命中项变化，页头要重新量
                        self.page.remeasure_items(0..1);
                    }
                    self.page.scroll_to(ListOffset {
                        item_ix: 0,
                        offset_in_item: px(0.),
                    });
                    self.page_query.clone_from(&query);
                }
                if breakpoint != self.page_breakpoint {
                    // 断点变化后页头会重排；保留旧高度当提示，等它可见时重新量
                    self.page.remeasure_items(0..1);
                    self.page_breakpoint = breakpoint;
                }
                if selecting != self.page_selecting {
                    // 批量操作条出现 / 消失会改变页头高度
                    self.page.remeasure_items(0..1);
                    self.page_selecting = selecting;
                }
            } else {
                self.page = ListState::new(count + 1, ListAlignment::Top, overdraw)
                    .with_uniform_item_height(px(env.row_height));
                self.page_rows = count;
                self.page_query.clone_from(&query);
                self.page_breakpoint = breakpoint;
                self.page_selecting = selecting;
            }
            self.page_head = Some(head.clone());

            let page = self.page.clone();
            let library = self.library.clone();
            let covers = self.covers.clone();
            let shell = self.shell.clone();
            let search_page = self.search_page.clone();
            let list_weak = list_weak.clone();
            let selected = Rc::clone(&selected);
            let toolbar = toolbar_state.clone();
            let page_query = query.clone();
            let scope_label = scope.clone();
            let page_body = list(page.clone(), move |ix, _window, cx| {
                if ix == 0 {
                    let mut header = div().flex_none().flex().flex_col().w_full();
                    match &head {
                        PageHead::Search => header = header.child(search_page.clone()),
                        PageHead::Scope(_) => {
                            if let Some(info) = hero.as_ref() {
                                header = header.child(detail::hero(
                                    info,
                                    library.clone(),
                                    &covers,
                                    shell.clone(),
                                    cx,
                                ));
                            }
                        }
                    }
                    header = header.child(chrome(
                        &library,
                        list_weak.clone(),
                        env,
                        &toolbar,
                        selecting,
                        chosen_count,
                        Rc::clone(&selected),
                        count,
                        collection,
                    ));
                    if count == 0 {
                        header = header.child(empty_body(
                            loading,
                            total,
                            0,
                            &library,
                            &page_query,
                            scope_label.clone(),
                            from_list,
                        ));
                    }
                    return header.into_any_element();
                }

                let index = ix - 1;
                let Some(track) = library
                    .read(cx)
                    .visible_tracks(index..index + 1)
                    .into_iter()
                    .next()
                else {
                    return div().into_any_element();
                };
                let state = row::RowState::of(track.id, playing, focus);
                let picked = selected.contains(&track.id);
                row::row(
                    index, &track, state, picked, env, &library, &list_weak, &covers, cx,
                )
            })
            .size_full()
            .into_any_element();

            return div()
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .child(div().flex_1().min_h_0().relative().child(page_body).child(
                    crate::common::scroll::scrollbar(&self.page, "track-page-scrollbar"),
                ))
                .into_any_element();
        }

        self.page_head = None;
        let header = chrome(
            &self.library,
            list_weak.clone(),
            env,
            &toolbar_state,
            selecting,
            chosen_count,
            Rc::clone(&self.selected),
            count,
            collection,
        );

        let body: AnyElement = if count == 0 {
            empty_body(loading, total, 0, &self.library, &query, scope, from_list)
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

/// 工具条、批量操作条与列头，纯列表模式固定在顶部，合成页里属于页头那一项。
fn chrome(
    library: &Entity<LibraryEntity>,
    list: WeakEntity<TrackList>,
    env: RowEnv,
    state: &toolbar::ToolbarState,
    selecting: bool,
    chosen_count: usize,
    selected: Rc<HashSet<TrackId>>,
    rows: usize,
    collection: Option<CollectionId>,
) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .child(toolbar::toolbar(
            library.clone(),
            list.clone(),
            state.clone(),
        ))
        .when(selecting, |column| {
            column.child(batch::batch_bar(batch::BatchBar {
                chosen_count,
                selected,
                rows,
                collection,
                library: library.clone(),
                list,
            }))
        })
        .child(row::column_header(&env))
        .into_any_element()
}

/// 曲目数为 0 时的占位内容。
fn empty_body(
    loading: bool,
    total: usize,
    count: usize,
    library: &Entity<LibraryEntity>,
    query: &str,
    scope: Option<String>,
    from_list: bool,
) -> AnyElement {
    if loading {
        empty::loading_library().into_any_element()
    } else if total == 0 {
        empty::empty_library(library.clone()).into_any_element()
    } else if count == 0 {
        if query.trim().is_empty() {
            empty::empty_scope(library.clone(), scope, from_list).into_any_element()
        } else {
            empty::no_matches(library.clone(), total, query).into_any_element()
        }
    } else {
        div().into_any_element()
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
