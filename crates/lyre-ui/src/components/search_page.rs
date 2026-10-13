//! 搜索页：未输入时画探索网格（从专辑分组派生），输入中画「最佳结果」+「艺人 / 专辑 / 歌单」区块。
//! 输入框只有顶栏那一份；歌曲结果由 [`crate::components::TrackList`]（曲库可见投影）承载，这里不另建列表。

use std::rc::Rc;

use lyre_core::{AlbumGroup, ArtistGroup, Track, TrackId};

use crate::components::detail::{HeroInfo, hero_for};
use crate::cover::CoverCache;
use crate::entities::{LibraryEntity, Scope, filter};
use crate::icon::{Glyph, icon, icon_inherit};
use crate::platform::*;
use crate::shell::{NavSection, ShellEntity};
use crate::theme;

/// 探索网格最多几块分类卡、每行几块（gpui 该版本没有 `flex_wrap`，按列数手工分行）。
const BROWSE_MAX: usize = 12;
const BROWSE_COLS: usize = 4;
const BROWSE_TILE_H: f32 = 76.0;
/// 每个分区块最多画几行。
const BLOCK_MAX: usize = 4;
/// 最佳结果卡宽度与封面边长。
const TOP_W: f32 = 320.0;
const TOP_COVER: f32 = 120.0;
/// 艺人 / 专辑区块行的封面边长。
const ROW_THUMB: f32 = 36.0;
/// 一个分组栏的最小可读宽度：再窄，栏头「艺术家」这五个字本身就要折行。
const RESULT_COL_MIN_W: f32 = 150.0;
/// 「最佳结果」与三个分组栏并排所需的最小可用宽度：
/// `TOP_W` + 一条间距 + 三栏各 `RESULT_COL_MIN_W` + 栏间两条间距 = 842px。
/// 达不到就让「最佳结果」独占一行。
const RESULTS_SIDE_BY_SIDE_MIN: f32 =
    TOP_W + theme::GAP_XL + RESULT_COL_MIN_W * 3.0 + theme::GAP_XL * 2.0;

pub struct SearchPage {
    library: Entity<LibraryEntity>,
    shell: Entity<ShellEntity>,
    covers: Entity<CoverCache>,
    _subs: Vec<Subscription>,
}

impl SearchPage {
    pub fn new(
        library: Entity<LibraryEntity>,
        shell: Entity<ShellEntity>,
        covers: Entity<CoverCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&library, |_, _, cx| cx.notify());
        let observe_covers = cx.observe(&covers, |_, _, cx| cx.notify());

        Self {
            library,
            shell,
            covers,
            _subs: vec![observe, observe_covers],
        }
    }

    /// 未搜索时给引导语而不是「找到 0 / 24 首」—— 后者看起来像搜索坏了。
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
        let summary = self.summary(cx);

        // 主区左上角浮着返回按钮（`root::floating_back`），标题不让开就会被压在下面；让位量与它同源于 `root::HEADER_BACK_INSET`。
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

        let mut page = div()
            .id("search-page")
            .flex_none()
            .flex()
            .flex_col()
            .w_full()
            .child(header);

        page = if searching {
            page.child(self.results_body(&query, cx))
        } else {
            page.child(self.browse_grid(cx))
        };

        page.into_any_element()
    }
}

impl SearchPage {
    /// 未输入时的探索网格：分类卡从专辑分组派生 —— 曲库没有流派元数据。
    fn browse_grid(&self, cx: &mut Context<Self>) -> AnyElement {
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

        let tiles: Vec<AlbumGroup> = groups.iter().take(BROWSE_MAX).cloned().collect();
        let mut grid = div().flex_none().flex().flex_col().gap(px(theme::GAP_MD));
        for (row_ix, chunk) in tiles.chunks(BROWSE_COLS).enumerate() {
            let mut row = div().flex_none().flex().gap(px(theme::GAP_MD));
            for (col, group) in chunk.iter().enumerate() {
                row = row.child(self.browse_tile(row_ix * BROWSE_COLS + col, group));
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
                    .text_size(px(theme::FONT_MICRO))
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

    /// 输入中的结果区：左侧最佳结果，右侧艺人 / 专辑 / 歌单分区块。
    fn results_body(&self, query: &str, cx: &mut Context<Self>) -> AnyElement {
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
                // 歌单也按名字匹配。名字先由 `label_for` 问一遍（收藏是虚拟歌单，不在 `Collections` 里但搜得到），命中之后才要头图数据。
                let list_matches: Vec<(Scope, HeroInfo)> = library
                    .list_scopes()
                    .into_iter()
                    .filter_map(|scope| {
                        let name = library.label_for(&scope)?;
                        if !filter::matches_name(&name, &terms) {
                            return None;
                        }
                        hero_for(library, &scope).map(|info| (scope, info))
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
                    list_matches.len(),
                    artist_matches,
                    album_matches,
                    list_matches,
                    top,
                )
            });

        if total == 0 && artist_total == 0 && album_total == 0 && list_total == 0 {
            return no_results().into_any_element();
        }

        let artist_rows: Vec<MiniRow> = artists
            .iter()
            .take(BLOCK_MAX)
            .map(MiniRow::from_artist)
            .collect();
        let album_rows: Vec<MiniRow> = albums
            .iter()
            .take(BLOCK_MAX)
            .map(MiniRow::from_album)
            .collect();

        // 三个分组栏。横向还是纵向撑满取决于下面走哪一支，所以 `flex_1` / `w_full` 留到分支里加。
        let groups = div()
            .flex()
            .min_w_0()
            .gap(px(theme::GAP_XL))
            .items_start()
            .child(self.group_column("艺术家", NavSection::Artists, artist_total, artist_rows, cx))
            .child(self.group_column("专辑", NavSection::Albums, album_total, album_rows, cx))
            .child(self.list_column(lists.into_iter().take(BLOCK_MAX).collect(), cx));

        let shell_row = |children: Div| {
            div()
                .flex_none()
                .flex()
                .w_full()
                .gap(px(theme::GAP_XL))
                .px(px(theme::GAP_LG))
                .pb(px(theme::GAP_MD))
                .items_start()
                .child(children)
        };

        // 没有命中的曲目时不留空的最佳结果栏。
        let Some(track) = top else {
            return shell_row(groups.flex_1()).into_any_element();
        };

        // `center_width` 已经扣掉侧栏与右面板，这里不能再减一次。
        let usable = self.shell.read(cx).center_width() - theme::GAP_LG * 2.0;
        if usable >= RESULTS_SIDE_BY_SIDE_MIN {
            shell_row(
                div()
                    .flex_none()
                    .flex()
                    .min_w_0()
                    .gap(px(theme::GAP_XL))
                    .items_start()
                    .child(self.top_column(Some(track), cx))
                    .child(groups.flex_1()),
            )
            .into_any_element()
        } else {
            // 放不下时「最佳结果」独占一行，三个分组栏分到整条宽度 —— 并排的话每栏只剩 57px。
            div()
                .flex_none()
                .flex()
                .flex_col()
                .w_full()
                .gap(px(theme::GAP_MD))
                .px(px(theme::GAP_LG))
                .pb(px(theme::GAP_MD))
                .child(self.top_column(Some(track), cx))
                .child(groups.w_full())
                .into_any_element()
        }
    }

    /// 歌单区块：只列命中的歌单，没有「查看全部」。
    fn list_column(&self, rows: Vec<(Scope, HeroInfo)>, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(theme::GAP_XS))
            .child(plain_header("歌单"));

        if rows.is_empty() {
            col = col.child(
                div()
                    .px(px(theme::GAP_SM))
                    .py(px(theme::GAP_MD))
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(theme::text_faint())
                    .child("没有匹配"),
            );
        } else {
            for (index, (scope, info)) in rows.into_iter().enumerate() {
                col = col.child(self.list_row(index, scope, info, cx));
            }
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

    fn top_column(&self, track: Option<Track>, cx: &mut App) -> AnyElement {
        let mut col = div()
            .flex_none()
            .w(px(TOP_W))
            .flex()
            .flex_col()
            .gap(px(theme::GAP_MD))
            .child(plain_header("最佳结果"));
        if let Some(track) = track {
            col = col.child(self.top_card(&track, cx));
        }
        col.into_any_element()
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

        if rows.is_empty() {
            col = col.child(
                div()
                    .px(px(theme::GAP_SM))
                    .py(px(theme::GAP_MD))
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(theme::text_faint())
                    .child("没有匹配"),
            );
        } else {
            for (index, row) in rows.into_iter().enumerate() {
                col = col.child(self.mini_row(section, index, row, cx));
            }
        }
        col.into_any_element()
    }

    /// 区块头：「查看全部」只在匹配数超过展示上限时出现，点了切到对应区段。
    /// 注意 `AlbumGrid` 目前不读搜索词，切过去是全量网格 —— 这里只保证入口可达，不假装它已过滤。
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

/// 艺人 / 专辑区块里的一行。
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

/// 行 / 卡片的小封面。取图统一走 [`crate::common::cover::cover_image`]，这里只画那一格。
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
}
