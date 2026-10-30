//! 搜索页：未输入时画探索网格（从专辑分组派生），输入中画「最佳结果」+「艺人 / 专辑 / 歌单」区块。
//! 输入框只有顶栏那一份；歌曲结果由 [`crate::components::TrackList`]（曲库可见投影）承载，这里不另建列表。

use std::collections::HashSet;
use std::rc::Rc;

use lyre_core::{AlbumGroup, ArtistGroup, Track, TrackId};

use crate::components::detail::{HeroInfo, hero_for};
use crate::cover::{CoverCache, CoverCacheEvent};
use crate::entities::{LibraryEntity, Scope, filter};
use crate::icon::{Glyph, icon, icon_inherit};
use crate::platform::*;
use crate::shell::{NavSection, ShellEntity};
use crate::theme;

/// 探索网格最多几块分类卡；列数按可用宽度反推（2 / 3 / 4），行数由它切。
const BROWSE_MAX: usize = 12;
/// 探索卡的最小可读宽度。再窄，卡上的专辑名只剩一个字。
const BROWSE_TILE_MIN_W: f32 = 150.0;
const BROWSE_TILE_H: f32 = 76.0;
const BLOCK_MAX: usize = 4;
const TOP_W: f32 = 320.0;
const TOP_COVER: f32 = 120.0;
const ROW_THUMB: f32 = 36.0;
/// 一个分组栏的最小可读宽度：再窄，栏头「艺术家」本身就要折行。
const RESULT_COL_MIN_W: f32 = 150.0;
/// 三个分组栏并排所需最小宽度：三栏各 `RESULT_COL_MIN_W` + 栏间两条间距。
/// 达不到就退成单列。
const GROUPS_ROW_MIN_W: f32 = RESULT_COL_MIN_W * 3.0 + theme::GAP_XL * 2.0;

/// 探索网格每行几块：按可用宽度反推，夹在 2–4 列之间。
fn browse_columns(usable: f32) -> usize {
    let gap = theme::GAP_MD;
    let columns = ((usable + gap) / (BROWSE_TILE_MIN_W + gap)).floor() as usize;
    columns.clamp(2, 4)
}

/// 分组栏一行放几栏：宽到能放下三栏才并排，否则单列。
fn group_columns(usable: f32, groups: usize) -> usize {
    if groups > 0 && usable >= GROUPS_ROW_MIN_W {
        groups
    } else {
        1
    }
}

/// 最佳结果与分组栏能否并排：按实际命中的分组数算，空分组不占宽，也不硬挤。
fn results_side_by_side(usable: f32, groups: usize) -> bool {
    groups > 0
        && usable
            >= TOP_W
                + theme::GAP_XL
                + groups as f32 * RESULT_COL_MIN_W
                + (groups - 1) as f32 * theme::GAP_XL
}

pub struct SearchPage {
    library: Entity<LibraryEntity>,
    shell: Entity<ShellEntity>,
    covers: Entity<CoverCache>,
    /// 搜索结果的缓存；查询词、可见投影或曲库数据没变就不重新匹配全部分组。
    results: Option<SearchCache>,
    _subs: Vec<Subscription>,
}

#[derive(Clone, PartialEq, Eq)]
struct SearchKey {
    query: String,
    visible_generation: u64,
    data_revision: u64,
}

struct SearchCache {
    key: SearchKey,
    results: SearchResults,
    /// 当前结果里会用到封面的 id；封面完成事件据此过滤。
    cover_ids: HashSet<TrackId>,
}

/// 一帧搜索结果需要的全部数据，分组只保留要画的前几项。
struct SearchResults {
    total: usize,
    artist_total: usize,
    album_total: usize,
    list_total: usize,
    artists: Vec<ArtistGroup>,
    albums: Vec<AlbumGroup>,
    lists: Vec<(Scope, HeroInfo)>,
    top: Option<Track>,
}

impl SearchPage {
    pub fn new(
        library: Entity<LibraryEntity>,
        shell: Entity<ShellEntity>,
        covers: Entity<CoverCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&library, |_, _, cx| cx.notify());
        // 只对当前结果里等的那几张封面重绘：整页 observe 会让每次解码都重算分组与 Hero。
        let on_cover = cx.subscribe(&covers, |this, _, event: &CoverCacheEvent, cx| {
            let relevant = match event {
                CoverCacheEvent::Ready(key) => this
                    .results
                    .as_ref()
                    .is_some_and(|cache| cache.cover_ids.contains(&key.track_id())),
                CoverCacheEvent::Evicted => true,
            };
            if relevant {
                cx.notify();
            }
        });

        Self {
            library,
            shell,
            covers,
            results: None,
            _subs: vec![observe, on_cover],
        }
    }

    fn sync_results(&mut self, query: &str, cx: &mut App) {
        let key = {
            let library = self.library.read(cx);
            SearchKey {
                query: query.to_owned(),
                visible_generation: library.visible_generation(),
                data_revision: library.data_revision(),
            }
        };
        if self.results.as_ref().is_some_and(|cache| cache.key == key) {
            return;
        }

        let terms = filter::parse_query(query);
        let (total, artist_total, album_total, list_total, artists, albums, lists, top) =
            App::update_entity(cx, &self.library, |library, _| {
                let total = library.visible_len();
                let all_artists = library.artists();
                let all_albums = library.albums();

                // 分组按名称匹配，与曲目匹配同口径（`filter::matches_name`）。
                // 分区块只画前 `BLOCK_MAX` 个，页头要的是命中总数，所以记命中下标而不是拷贝分组。
                let artist_hits: Vec<usize> = all_artists
                    .iter()
                    .enumerate()
                    .filter(|(_, group)| filter::matches_name(&group.name, &terms))
                    .map(|(index, _)| index)
                    .collect();
                let album_hits: Vec<usize> = all_albums
                    .iter()
                    .enumerate()
                    .filter(|(_, group)| filter::matches_name(&group.title, &terms))
                    .map(|(index, _)| index)
                    .collect();
                let artist_matches: Vec<ArtistGroup> = artist_hits
                    .iter()
                    .take(BLOCK_MAX)
                    .map(|&index| all_artists[index].clone())
                    .collect();
                let album_matches: Vec<AlbumGroup> = album_hits
                    .iter()
                    .take(BLOCK_MAX)
                    .map(|&index| all_albums[index].clone())
                    .collect();
                // 收藏是虚拟歌单，不在 `Collections` 里，匹配要先问 `label_for` 拿名字。
                let mut list_total = 0usize;
                let list_matches: Vec<(Scope, HeroInfo)> = library
                    .list_scopes()
                    .into_iter()
                    .filter_map(|scope| {
                        let name = library.label_for(&scope)?;
                        if !filter::matches_name(&name, &terms) {
                            return None;
                        }
                        let info = hero_for(library, &scope)?;
                        list_total += 1;
                        (list_total <= BLOCK_MAX).then_some((scope, info))
                    })
                    .collect();

                let top = if total > 0 {
                    library.visible_tracks(0..1).into_iter().next()
                } else {
                    None
                };

                (
                    total,
                    artist_hits.len(),
                    album_hits.len(),
                    list_total,
                    artist_matches,
                    album_matches,
                    list_matches,
                    top,
                )
            });

        let mut cover_ids = HashSet::new();
        for group in &artists {
            cover_ids.insert(group.representative);
        }
        for group in &albums {
            cover_ids.insert(group.representative);
        }
        for (_, info) in &lists {
            if let Some(id) = info.cover_id {
                cover_ids.insert(id);
            }
            if let Some(file) = &info.cover_file {
                cover_ids.insert(TrackId::from_path(file));
            }
        }
        if let Some(track) = &top {
            cover_ids.insert(track.id);
        }

        self.results = Some(SearchCache {
            key,
            results: SearchResults {
                total,
                artist_total,
                album_total,
                list_total,
                artists,
                albums,
                lists,
                top,
            },
            cover_ids,
        });
    }

    fn summary(&self, cx: &App) -> String {
        let library = self.library.read(cx);
        let query = library.query().trim();
        if query.is_empty() {
            format!(
                "在 {} 首曲目里查找标题、艺术家或专辑",
                crate::common::thousands(library.len())
            )
        } else {
            format!("「{query}」的结果")
        }
    }
}

impl Render for SearchPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.library.read(cx).query().to_string();
        let searching = crate::entities::searching(&query);
        let loading = self.library.read(cx).loading();
        let summary = self.summary(cx);
        // 主区宽度已扣掉侧栏与停靠的面板，这里再扣页面左右内边距。
        let usable = (self.shell.read(cx).center_width() - theme::GAP_LG * 2.0).max(0.0);

        // 主区左上角浮着返回按钮（`root::floating_back`），标题不让开就会被压在下面；
        // 让位量与它同源于 `root::HEADER_BACK_INSET`。
        let header_top = if self.shell.read(cx).can_go_back() {
            crate::root::HEADER_BACK_INSET + theme::GAP_MD
        } else {
            theme::GAP_XL
        };

        let header = div()
            .flex_none()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(theme::GAP_SM))
            .pt(px(header_top))
            .pb(px(theme::GAP_MD))
            .px(px(theme::GAP_LG))
            .child(
                div()
                    .flex_none()
                    .text_size(px(theme::FONT_HERO))
                    .font_weight(theme::WEIGHT_TITLE)
                    .text_color(theme::text())
                    .child("搜索"),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(if searching {
                        theme::text()
                    } else {
                        theme::text_muted()
                    })
                    .child(summary),
            );

        // 有歌曲命中时这块是 TrackList 的页头：大标题和「结果」摘要让位，歌曲行才能尽早上屏。
        // 空搜索或只有分组命中时仍保留标题。
        let track_results = searching && self.library.read(cx).visible_len() > 0;

        let mut page = div()
            .id("search-page")
            .flex_none()
            .flex()
            .flex_col()
            .w_full();
        page = if track_results {
            page.pt(px(header_top))
        } else {
            page.child(header)
        };

        page = if searching {
            page.child(self.results_body(&query, usable, cx))
        } else if loading {
            page.child(loading_state())
        } else {
            page.child(self.browse_grid(usable, cx))
        };

        page.into_any_element()
    }
}

impl SearchPage {
    /// 未输入时的探索网格：分类卡从专辑分组派生 —— 曲库没有流派元数据。
    fn browse_grid(&self, usable: f32, cx: &mut Context<Self>) -> AnyElement {
        let groups = App::update_entity(cx, &self.library, |library, _| library.albums());
        if groups.is_empty() {
            return div()
                .flex_none()
                .px(px(theme::GAP_LG))
                .pb(px(theme::GAP_XL))
                .text_size(px(theme::FONT_BODY))
                .text_color(theme::text_muted())
                .child("曲库还是空的 —— 导入一些音乐后，这里会出现可浏览的分类。")
                .into_any_element();
        }

        let columns = browse_columns(usable);
        let tiles: Vec<AlbumGroup> = groups.iter().take(BROWSE_MAX).cloned().collect();
        let mut grid = div().flex_none().flex().flex_col().gap(px(theme::GAP_MD));
        for (row_ix, chunk) in tiles.chunks(columns).enumerate() {
            let mut row = div().flex_none().flex().gap(px(theme::GAP_MD));
            for (col, group) in chunk.iter().enumerate() {
                row = row.child(self.browse_tile(row_ix * columns + col, group));
            }
            grid = grid.child(row);
        }

        div()
            .flex_none()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(theme::GAP_MD))
            .px(px(theme::GAP_LG))
            .pb(px(theme::GAP_XL))
            .child(
                div()
                    .flex_none()
                    .text_size(px(theme::FONT_TITLE))
                    .font_weight(theme::WEIGHT_TITLE)
                    .text_color(theme::text())
                    .child("浏览全部"),
            )
            .child(grid)
            .into_any_element()
    }

    fn browse_tile(&self, index: usize, group: &AlbumGroup) -> AnyElement {
        let color = crate::common::cover::monogram_color(group.title.chars().next().unwrap_or('?'));
        let title = group.title.clone();
        let meta = format!("{} 首", group.track_count());
        let id = group.representative;
        let library = self.library.clone();
        let shell = self.shell.clone();

        div()
            .id(("search-browse", index))
            .flex_1()
            .min_w_0()
            .h(px(BROWSE_TILE_H))
            .relative()
            .overflow_hidden()
            .rounded(px(theme::RADIUS_MD))
            .bg(color)
            .cursor_pointer()
            .hover(|style| style.opacity(0.88))
            .tooltip(crate::common::tooltip::builder(group.title.clone()))
            .on_click({
                let title = title.clone();
                let library = library.clone();
                let shell = shell.clone();
                move |_, _, cx| {
                    crate::shell::enter_scope_from_here(
                        Scope::Album(title.clone()),
                        &shell,
                        &library,
                        cx,
                    );
                }
            })
            .child(
                div()
                    .absolute()
                    .left(px(theme::GAP_MD))
                    .right(px(theme::GAP_MD))
                    .top(px(theme::GAP_MD))
                    .text_size(px(theme::FONT_BODY))
                    .font_weight(theme::WEIGHT_SUBTITLE)
                    .text_color(rgb(0xffffff))
                    .truncate()
                    .child(group.title.clone()),
            )
            .child(
                div()
                    .absolute()
                    .left(px(theme::GAP_MD))
                    .bottom(px(theme::GAP_SM))
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(rgb(0xffffff).opacity(0.75))
                    .child(meta),
            )
            .context_menu(crate::common::menu::group_context_menu(
                self.library.clone(),
                crate::common::menu::GroupKind::Album,
                id,
                title.clone(),
                Rc::new(move |cx: &mut App| {
                    crate::shell::enter_scope_from_here(
                        Scope::Album(title.clone()),
                        &shell,
                        &library,
                        cx,
                    );
                }),
            ))
            .into_any_element()
    }

    fn results_body(&mut self, query: &str, usable: f32, cx: &mut Context<Self>) -> AnyElement {
        self.sync_results(query, cx);
        let Some(cache) = self.results.as_ref() else {
            return div().into_any_element();
        };
        let results = &cache.results;
        let (total, artist_total, album_total, list_total) = (
            results.total,
            results.artist_total,
            results.album_total,
            results.list_total,
        );

        if total == 0 && artist_total == 0 && album_total == 0 && list_total == 0 {
            return no_results().into_any_element();
        }

        // 只画命中的分组：空分组连同「没有匹配」不再占首屏高度。
        let mut blocks: Vec<AnyElement> = Vec::new();
        if artist_total > 0 {
            let rows = results.artists.iter().map(MiniRow::from_artist).collect();
            blocks.push(self.group_column("艺术家", NavSection::Artists, artist_total, rows, cx));
        }
        if album_total > 0 {
            let rows = results.albums.iter().map(MiniRow::from_album).collect();
            blocks.push(self.group_column("专辑", NavSection::Albums, album_total, rows, cx));
        }
        if list_total > 0 {
            blocks.push(self.list_column(results.lists.clone(), cx));
        }
        let groups_count = blocks.len();
        let groups = if group_columns(usable, groups_count) == 1 {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(theme::GAP_XL))
                .children(blocks)
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .gap(px(theme::GAP_XL))
                .items_start()
                .children(blocks)
        };

        let pad = |body: Div| {
            body.flex_none()
                .flex()
                .w_full()
                .px(px(theme::GAP_LG))
                .pb(px(theme::GAP_MD))
        };

        // 没有命中的曲目时不留空的最佳结果栏，分组栏独占整行。
        let Some(track) = results.top.clone() else {
            return pad(div().flex_col())
                .child(groups.w_full())
                .into_any_element();
        };

        if results_side_by_side(usable, groups_count) {
            return pad(div().gap(px(theme::GAP_XL)).items_start())
                .child(self.top_column(&track, false, cx))
                .child(groups)
                .into_any_element();
        }

        // 放不下时「最佳结果」整行，分组栏在下方；窄屏分组栏还是单列。
        pad(div().flex_col().gap(px(theme::GAP_MD)))
            .child(self.top_column(&track, true, cx))
            .when(groups_count > 0, |body| body.child(groups.w_full()))
            .into_any_element()
    }

    /// 歌单区块：只列命中的歌单，没有「查看全部」。调用方保证 `rows` 非空。
    fn list_column(&self, rows: Vec<(Scope, HeroInfo)>, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(theme::GAP_XS))
            .child(plain_header("歌单"));

        for (index, (scope, info)) in rows.into_iter().enumerate() {
            col = col.child(self.list_row(index, scope, info, cx));
        }
        col.into_any_element()
    }

    fn list_row(&self, index: usize, scope: Scope, info: HeroInfo, cx: &mut App) -> AnyElement {
        let cover = crate::common::cover::list_cover(
            &self.covers,
            &self.library,
            ROW_THUMB,
            theme::RADIUS_MD,
            info.cover_file.as_deref(),
            info.cover_id,
            &info.title,
            cx,
        );
        let library = self.library.clone();
        let shell = self.shell.clone();
        // 只有自建歌单才有可改的地方：收藏的成员由爱心决定，不能改名也不能删。
        let editable = match &scope {
            Scope::Collection(id) => Some(*id),
            _ => None,
        };
        let menu_title = info.title.clone();
        let tip = if info.meta.is_empty() {
            info.title.clone()
        } else {
            format!("{} · {}", info.title, info.meta)
        };

        let row = div()
            .id(("search-list-row", index))
            .flex_none()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(theme::GAP_SM))
            .p(px(theme::GAP_XS))
            .rounded(px(theme::RADIUS_MD))
            .cursor_pointer()
            .hover(|style| style.bg(theme::hover()))
            .tooltip(crate::common::tooltip::builder(tip))
            .on_click(move |_, _, cx| {
                // 搜索词由 `enter_scope` 清掉；`enter_scope_from_here` 先记历史，顶栏 ← 能退回搜索页。
                crate::shell::enter_scope_from_here(scope.clone(), &shell, &library, cx);
            })
            .child(cover)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(px(theme::FONT_BODY))
                            .text_color(theme::text())
                            .truncate()
                            .child(info.title),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(theme::text_faint())
                            .truncate()
                            .child(info.meta),
                    ),
            );

        match editable {
            Some(id) => row
                .context_menu(crate::components::sidebar::collection_menu(
                    self.library.clone(),
                    id,
                    menu_title,
                ))
                .into_any_element(),
            None => row.into_any_element(),
        }
    }

    /// `fill`：并排时钉 `TOP_W`，换行独占时铺满整行，避免右侧留一大块空白。
    fn top_column(&self, track: &Track, fill: bool, cx: &mut App) -> AnyElement {
        let mut col = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(theme::GAP_MD))
            .child(plain_header("最佳结果"));
        col = if fill { col.w_full() } else { col.w(px(TOP_W)) };
        col.child(self.top_card(track, cx)).into_any_element()
    }

    fn top_card(&self, track: &Track, cx: &mut App) -> AnyElement {
        let id = track.id;
        let title = track.title.clone();
        let subtitle = track
            .artist
            .clone()
            .filter(|a| !a.trim().is_empty())
            .or_else(|| track.album.clone())
            .unwrap_or_default();
        let tip = if subtitle.is_empty() {
            title.clone()
        } else {
            format!("{title} · {subtitle}")
        };
        let cover = thumb(
            &self.covers,
            &self.library,
            id,
            TOP_COVER,
            theme::RADIUS_MD,
            false,
            cx,
        );
        let library = self.library.clone();

        div()
            .id("search-top-card")
            .flex_none()
            .w_full()
            .flex()
            .items_center()
            .gap(px(theme::GAP_MD))
            .p(px(theme::GAP_MD))
            .rounded(px(theme::RADIUS_CARD))
            .bg(theme::elevated())
            .cursor_pointer()
            .hover(|style| style.bg(theme::border()))
            .tooltip(crate::common::tooltip::builder(tip))
            .on_click(move |_, _, cx| {
                library.update(cx, |library, cx| {
                    library.activate_in_view(id, cx);
                });
            })
            .child(
                div().relative().flex_none().child(cover).child(
                    div()
                        .absolute()
                        .right(px(theme::GAP_SM))
                        .bottom(px(theme::GAP_SM))
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(theme::control())
                        .border(px(2.))
                        .border_color(theme::bg())
                        .child(icon(Glyph::Play, 16., theme::text_on_control())),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(theme::GAP_XS))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(px(theme::FONT_TITLE))
                            .font_weight(theme::WEIGHT_TITLE)
                            .text_color(theme::text())
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(theme::text_muted())
                            .truncate()
                            .child(subtitle),
                    ),
            )
            .context_menu(crate::common::menu::track_context_menu(
                self.library.clone(),
                id,
                crate::common::menu::TrackMenuHooks::none(),
            ))
            .into_any_element()
    }

    fn group_column(
        &self,
        title: &'static str,
        section: NavSection,
        total: usize,
        rows: Vec<MiniRow>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut col = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(theme::GAP_XS))
            .child(self.block_header(title, section, total));

        for (index, row) in rows.into_iter().enumerate() {
            col = col.child(self.mini_row(section, index, row, cx));
        }
        col.into_any_element()
    }

    /// `AlbumGrid` 不读搜索词，「查看全部」切过去是全量网格 —— 这里只保证入口可达，不假装已过滤。
    fn block_header(&self, title: &'static str, section: NavSection, total: usize) -> AnyElement {
        let mut row = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(theme::GAP_MD))
            .child(plain_header(title));

        if total > BLOCK_MAX {
            let shell = self.shell.clone();
            let library = self.library.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("search-all-{title}")))
                    .flex_none()
                    .cursor_pointer()
                    .text_size(px(theme::FONT_CAPTION))
                    .font_weight(theme::WEIGHT_BUTTON)
                    .text_color(theme::text())
                    .hover(|style| style.text_color(theme::control_hover()))
                    .on_click(move |_, _, cx| crate::shell::go_to(section, &shell, &library, cx))
                    .child("查看全部"),
            );
        }
        row.into_any_element()
    }

    fn mini_row(
        &self,
        section: NavSection,
        index: usize,
        row: MiniRow,
        cx: &mut App,
    ) -> AnyElement {
        let is_artist = row.is_artist;
        let thumb = thumb(
            &self.covers,
            &self.library,
            row.id,
            ROW_THUMB,
            theme::RADIUS_MD,
            is_artist,
            cx,
        );
        let library = self.library.clone();
        let shell = self.shell.clone();
        let name = row.name.clone();
        let group_id = row.id;
        let tip = if row.meta.is_empty() {
            row.name.clone()
        } else {
            format!("{} · {}", row.name, row.meta)
        };

        div()
            .id((section_label(section), index))
            .flex_none()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(theme::GAP_SM))
            .p(px(theme::GAP_XS))
            .rounded(px(theme::RADIUS_MD))
            .cursor_pointer()
            .hover(|style| style.bg(theme::hover()))
            .tooltip(crate::common::tooltip::builder(tip))
            .on_click({
                let library = library.clone();
                let shell = shell.clone();
                let name = name.clone();
                move |_, _, cx| {
                    let scope = if is_artist {
                        Scope::Artist(name.clone())
                    } else {
                        Scope::Album(name.clone())
                    };
                    crate::shell::enter_scope_from_here(scope, &shell, &library, cx);
                }
            })
            .child(thumb)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(px(theme::FONT_BODY))
                            .text_color(theme::text())
                            .truncate()
                            .child(row.name),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(theme::text_faint())
                            .truncate()
                            .child(row.meta),
                    ),
            )
            .context_menu(crate::common::menu::group_context_menu(
                self.library.clone(),
                if is_artist {
                    crate::common::menu::GroupKind::Artist
                } else {
                    crate::common::menu::GroupKind::Album
                },
                group_id,
                name.clone(),
                Rc::new(move |cx: &mut App| {
                    let scope = if is_artist {
                        Scope::Artist(name.clone())
                    } else {
                        Scope::Album(name.clone())
                    };
                    crate::shell::enter_scope_from_here(scope, &shell, &library, cx);
                }),
            ))
            .into_any_element()
    }
}

struct MiniRow {
    id: TrackId,
    name: String,
    meta: String,
    is_artist: bool,
}

impl MiniRow {
    fn from_artist(group: &ArtistGroup) -> Self {
        Self {
            id: group.representative,
            name: group.name.clone(),
            meta: format!("{} 张专辑", group.album_count()),
            is_artist: true,
        }
    }

    fn from_album(group: &AlbumGroup) -> Self {
        let artist = group.display_artist();
        let meta = if artist.is_empty() {
            format!("{} 首", group.track_count())
        } else {
            format!("{} · {} 首", artist, group.track_count())
        };
        Self {
            id: group.representative,
            name: group.title.clone(),
            meta,
            is_artist: false,
        }
    }
}

fn plain_header(text: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .text_size(px(theme::FONT_BODY))
        .font_weight(theme::WEIGHT_SUBTITLE)
        .text_color(theme::text())
        .child(text)
}

/// 区块行的元素 id 前缀。用固定字符串而不是中文标题，避免 id 里出现会变的文案。
fn section_label(section: NavSection) -> &'static str {
    match section {
        NavSection::Artists => "search-artist-row",
        _ => "search-album-row",
    }
}

fn thumb(
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    id: TrackId,
    size: f32,
    radius: f32,
    circular: bool,
    cx: &mut App,
) -> AnyElement {
    let state = crate::common::cover::cover_state(covers, library, id, size, cx);
    let radius = if circular { theme::RADIUS_PILL } else { radius };

    let cell = div()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius))
        .bg(theme::elevated());

    match state {
        crate::common::cover::Cover::Ready(image) => cell
            .child(
                img(ImageSource::Render(image))
                    .size_full()
                    .rounded(px(radius))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element(),
        crate::common::cover::Cover::Missing => cell
            .border_1()
            .border_color(theme::border())
            .text_color(theme::text_faint())
            .child(icon_inherit(
                if circular {
                    Glyph::Artist
                } else {
                    Glyph::Music
                },
                16.,
            ))
            .into_any_element(),
        crate::common::cover::Cover::Pending => cell
            .child(crate::common::cover::skeleton(size, radius))
            .into_any_element(),
    }
}

fn loading_state() -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .px(px(theme::GAP_LG))
        .pb(px(theme::GAP_XL))
        .child(
            div()
                .text_size(px(theme::FONT_TITLE))
                .font_weight(theme::WEIGHT_TITLE)
                .text_color(theme::text())
                .child("正在读取曲库…"),
        )
        .child(
            div()
                .text_size(px(theme::FONT_BODY))
                .text_color(theme::text_muted())
                .child("窗口先显示，搜索与分类会在后台核对完成后出现。"),
        )
}

fn no_results() -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .px(px(theme::GAP_LG))
        .pb(px(theme::GAP_XL))
        .child(
            div()
                .text_size(px(theme::FONT_TITLE))
                .font_weight(theme::WEIGHT_TITLE)
                .text_color(theme::text())
                .child("没有找到结果"),
        )
        .child(
            div()
                .text_size(px(theme::FONT_BODY))
                .text_color(theme::text_muted())
                .child("换个关键词试试，或者检查拼写。"),
        )
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`platform::*` 里的 gpui `test` 过程宏会遮蔽内置 `#[test]`。
    use super::{browse_columns, group_columns, results_side_by_side};
    use crate::entities::searching;

    #[test]
    fn a_typed_query_puts_the_page_in_result_mode() {
        assert!(searching("城市"));
    }

    #[test]
    fn an_untouched_box_is_not_searching() {
        assert!(!searching(""));
        assert!(!searching("   "));
    }

    #[test]
    fn browse_grid_uses_two_to_four_columns_by_width() {
        assert_eq!(browse_columns(0.0), 2, "极窄也至少两列");
        assert_eq!(browse_columns(440.0), 2, "560px 窗口下的搜索页主区");
        assert_eq!(browse_columns(560.0), 3);
        assert_eq!(browse_columns(820.0), 4);
        assert_eq!(browse_columns(4000.0), 4, "超宽屏不再增加列数");
    }

    #[test]
    fn result_groups_go_single_column_when_three_would_be_cramped() {
        assert_eq!(
            group_columns(440.0, 3),
            1,
            "窄屏三栏每栏只剩约 130px，读不了"
        );
        assert_eq!(group_columns(520.0, 3), 3);
        assert_eq!(group_columns(520.0, 2), 2, "只命中两组就排两组，不硬凑三栏");
    }

    #[test]
    fn side_by_side_only_when_every_remaining_group_fits() {
        assert!(!results_side_by_side(440.0, 3));
        assert!(!results_side_by_side(440.0, 1), "320 + 间距 + 150 仍放不下");
        assert!(results_side_by_side(500.0, 1));
        assert!(results_side_by_side(850.0, 3));
        assert!(
            !results_side_by_side(850.0, 0),
            "没有分组可并排时不能让最佳结果被挤成半宽"
        );
    }
}
