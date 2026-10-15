//! 封面网格：专辑 / 艺术家两种分组共用一套渲染，用 `uniform_list` 虚拟化。
//! 每张卡都向 `CoverCache` 请求封面，一次性请求几百张会把 LRU 互相挤出、封面反复闪回占位。

use std::rc::Rc;

use lyre_core::{AlbumGroup, ArtistGroup, TrackId, format_duration};

use crate::common::thousands;
use crate::cover::CoverCache;
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::shell::{CARD_MAX_W, NavSection, ShellEntity};
use crate::theme;

/// 卡片间距，同时用作行间距。
const GAP: f32 = theme::GAP_MD;
const PAD: f32 = theme::GAP_LG;
/// 封面区最小高度，实际高度按卡宽算，见 `cover_height`。
const COVER_H_MIN: f32 = 132.0;
/// 封面区最大高度，等于 `CARD_MAX_W`，所以最宽的卡片正好是方形封面。
const COVER_H_MAX: f32 = CARD_MAX_W;
/// 行高按最坏情况给：`uniform_list` 不算内容高度，少了就溢出卡片下缘。
/// `66` = 标题 19 + 副标题 16 + 说明 15 + 两段 gap 16；`CARD_PAD` 常态占位（见 `card_item`）也要计入。
const CARD_PAD: f32 = theme::GAP_MD;
const CARD_H: f32 = COVER_H_MAX + 66.0 + CARD_PAD * 2.0;
const ROW_H: f32 = CARD_H + GAP;

/// 渲染一行卡片所需的共享依赖。
struct RowCtx<'a> {
    library: &'a Entity<LibraryEntity>,
    covers: &'a Entity<CoverCache>,
    grid: &'a WeakEntity<AlbumGrid>,
    /// 由 `ShellEntity::grid_card_width()` 推出，全网格共用一个值。
    cover_h: f32,
    cx: &'a mut App,
}

#[derive(Clone)]
pub struct Card {
    /// 用来取封面的曲目，不一定是第一首，见 `lyre_core::group` 的规则④
    id: TrackId,
    title: String,
    /// 副标题。艺术家卡留空 —— 否则总等于自己的标题。
    subtitle: String,
    /// 「12 首 · 42:15」这类说明
    meta: String,
    initial: Option<char>,
    kind: NavSection,
}

impl Card {
    fn from_album(group: &AlbumGroup) -> Self {
        let tracks = group.track_count();
        let meta = match group.total_duration() {
            Some(total) => format!("{} 首 · {}", thousands(tracks), format_duration(total)),
            None => format!("{} 首", thousands(tracks)),
        };
        Self {
            id: group.representative,
            title: group.title.clone(),
            subtitle: group.display_artist(),
            meta,
            initial: group.title.chars().next(),
            kind: NavSection::Albums,
        }
    }

    fn from_artist(group: &ArtistGroup) -> Self {
        let (tracks, albums) = (group.track_count(), group.album_count());
        let meta = match group.total_duration() {
            Some(total) => format!(
                "{} 首 · {} 张专辑 · {}",
                thousands(tracks),
                thousands(albums),
                format_duration(total)
            ),
            None => format!("{} 首 · {} 张专辑", thousands(tracks), thousands(albums)),
        };
        Self {
            id: group.representative,
            title: group.name.clone(),
            subtitle: String::new(),
            meta,
            initial: group.name.chars().next(),
            kind: NavSection::Artists,
        }
    }
}

/// 点开一张卡片。`RootView` 据此收窄曲库作用域并切回曲目视图。
#[derive(Clone, Debug)]
pub enum GridEvent {
    OpenAlbum(String),
    OpenArtist(String),
}

pub struct AlbumGrid {
    library: Entity<LibraryEntity>,
    shell: Entity<ShellEntity>,
    covers: Entity<CoverCache>,
    /// 网格行的派生缓存。失效键只需分区、列数、分组代次三个 —— 卡片内容只由分组结果决定。
    rows_cache: Option<RowsCache>,
    /// 滚动偏移。句柄不挂时 `uniform_list` 每次从头算，进专辑再返回会跳回开头。
    scroll: UniformListScrollHandle,
    _subs: Vec<Subscription>,
}

struct RowsCache {
    section: NavSection,
    columns: usize,
    generation: u64,
    rows: Rc<Vec<Vec<Card>>>,
}

impl EventEmitter<GridEvent> for AlbumGrid {}

impl AlbumGrid {
    pub fn new(
        library: Entity<LibraryEntity>,
        shell: Entity<ShellEntity>,
        covers: Entity<CoverCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe_library = cx.observe(&library, |_, _, cx| cx.notify());
        let observe_shell = cx.observe(&shell, |_, _, cx| cx.notify());
        let observe_covers = cx.observe(&covers, |_, _, cx| cx.notify());

        Self {
            library,
            shell,
            covers,
            rows_cache: None,
            scroll: UniformListScrollHandle::default(),
            _subs: vec![observe_library, observe_shell, observe_covers],
        }
    }

    /// 每行几列，由 `ShellEntity` 按可用宽度反推。
    fn columns(&self, cx: &App) -> usize {
        self.shell.read(cx).grid_columns()
    }

    /// 把分组切成行。封面每解码完一张就 notify 一次，重建整网格不该摊进每一帧。
    fn rows(&mut self, cx: &mut App) -> Rc<Vec<Vec<Card>>> {
        let section = self.shell.read(cx).nav_section();
        let columns = self.columns(cx).max(1);
        let generation =
            App::update_entity(cx, &self.library, |library, _| library.groups_generation());

        if let Some(cached) = &self.rows_cache
            && cached.section == section
            && cached.columns == columns
            && cached.generation == generation
        {
            return Rc::clone(&cached.rows);
        }

        let cards: Vec<Card> = match section {
            NavSection::Albums => App::update_entity(cx, &self.library, |library, _| {
                library.albums().iter().map(Card::from_album).collect()
            }),
            _ => App::update_entity(cx, &self.library, |library, _| {
                library.artists().iter().map(Card::from_artist).collect()
            }),
        };

        let rows = Rc::new(
            cards
                .chunks(columns)
                .map(<[Card]>::to_vec)
                .collect::<Vec<Vec<Card>>>(),
        );
        self.rows_cache = Some(RowsCache {
            section,
            columns,
            generation,
            rows: Rc::clone(&rows),
        });
        rows
    }
}

impl Render for AlbumGrid {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self.shell.read(cx).nav_section();
        let rows = self.rows(cx);
        let row_count = rows.len();
        let item_count = total_items(&rows);
        let columns = self.columns(cx);

        let header = grid_header(section, item_count, columns);

        if row_count == 0 {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .child(header)
                .child(div().flex_1().min_h_0().child(empty_grid()))
                .into_any_element();
        }

        // `uniform_list` 要求行等高：封面高度只能取布局层算好的卡宽，自适应会让虚拟列表错位。
        let cover_h = cover_height(self.shell.read(cx).grid_card_width());

        let library = self.library.clone();
        let covers = self.covers.clone();
        let grid = cx.weak_entity();

        let list = uniform_list("album-grid", row_count, move |range, _window, cx| {
            let visible: Vec<Vec<Card>> = rows[range.clone()].to_vec();

            visible
                .into_iter()
                .enumerate()
                .map(|(offset, cards)| {
                    grid_row(
                        range.start + offset,
                        &cards,
                        columns,
                        RowCtx {
                            library: &library,
                            covers: &covers,
                            grid: &grid,
                            cover_h,
                            cx,
                        },
                    )
                })
                .collect::<Vec<AnyElement>>()
        })
        .size_full()
        .pt(px(PAD))
        .px(px(PAD))
        // 句柄必须挂上：滚动条读的就是它，不挂则偏移恒为 0。
        .track_scroll(&self.scroll)
        .into_any_element();

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(div().flex_1().min_h_0().relative().child(list).child(
                crate::common::scroll::scrollbar(&self.scroll, "album-grid-scrollbar"),
            ))
            .into_any_element()
    }
}

/// 所有行里的卡片总数 —— 页头报的是它，不是 `rows.len()`。
fn total_items(rows: &[Vec<Card>]) -> usize {
    rows.iter().map(Vec::len).sum()
}

fn grid_header(section: NavSection, count: usize, columns: usize) -> impl IntoElement {
    let noun = match section {
        NavSection::Albums => "张专辑",
        _ => "位艺术家",
    };
    let summary = format!("{} · {}", title_for(section), thousands(count));

    let hint = format!("每行 {columns} 个 · 共 {} {noun}", thousands(count));

    // 网格页没有浮动返回按钮，左内边距不让位。
    let back_inset = theme::GAP_LG;
    let top_pad = theme::GAP_XL;

    div()
        .flex_none()
        .flex()
        .flex_col()
        .justify_end()
        .gap(px(theme::GAP_SM))
        .pt(px(top_pad))
        .pb(px(theme::GAP_LG))
        .pl(px(back_inset))
        .pr(px(theme::GAP_LG))
        .child(
            div()
                .text_size(px(theme::FONT_HERO))
                .font_weight(theme::WEIGHT_TITLE)
                .text_color(theme::text())
                .child(summary),
        )
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_muted())
                .child(hint),
        )
}

fn title_for(section: NavSection) -> &'static str {
    match section {
        NavSection::Albums => "专辑",
        _ => "艺术家",
    }
}

fn grid_row(index: usize, cards: &[Card], columns: usize, ctx: RowCtx<'_>) -> AnyElement {
    let RowCtx {
        library,
        covers,
        grid,
        cover_h,
        cx,
    } = ctx;
    let mut row = div()
        .id(("grid-row", index))
        .flex_none()
        .h(px(ROW_H))
        .flex()
        .justify_start()
        .gap(px(GAP));

    let filled = cards.len();
    for card in cards {
        row = row.child(card_item(card, library, covers, grid, cover_h, cx));
    }

    // 末行不满时补占位，否则剩下的 `flex_1` 会被拉宽成一行巨幅封面。
    for extra in filled..columns {
        row = row.child(filler(extra));
    }

    row.into_any_element()
}

/// 撑开一格但不画任何东西，用于补齐末行的空位。
fn filler(index: usize) -> impl IntoElement {
    div()
        .id(("grid-filler", index))
        .flex_1()
        .min_w_0()
        .h(px(CARD_H))
}

/// 封面高度跟着卡宽 clamp；固定高度会在宽卡上把封面拉成横幅。
fn cover_height(card_width: f32) -> f32 {
    card_width.clamp(COVER_H_MIN, COVER_H_MAX)
}

fn card_item(
    card: &Card,
    library: &Entity<LibraryEntity>,
    covers: &Entity<CoverCache>,
    grid: &WeakEntity<AlbumGrid>,
    cover_h: f32,
    cx: &mut App,
) -> impl IntoElement {
    let title = card.title.clone();
    let grid = grid.clone();
    let event = match card.kind {
        NavSection::Albums => GridEvent::OpenAlbum(title.clone()),
        // `Tracks` 分支不可达，这里只是给个明确兜底
        _ => GridEvent::OpenArtist(title.clone()),
    };

    let cover = cover_cell(covers, library, card, cover_h, cx);
    // group 也用标题：重复的 group 会让同名分组的两张卡一起点亮悬停态
    let group = SharedString::from(format!("card-{}", card.title));
    let title_for_play = card.title.clone();
    let track_id = card.id;

    div()
        // id 用标题而不是 `representative`：一首歌挂在多个艺术家下时，每个分组都选中同一首代表，
        // 同一行就会冒出两个相同 id —— debug 构建直接 panic。
        .id(SharedString::from(format!("album-card-{}", card.title)))
        .group(group.clone())
        .flex_1()
        .min_w_0()
        // 列数撞到 `MAX_GRID_COLUMNS` 后不再增长，宽屏上的卡宽靠这条截断
        .max_w(px(CARD_MAX_W))
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        // 内边距必须常态就有：hover 时才加会让卡片在鼠标移入的瞬间抖一下
        .p(px(theme::GAP_MD))
        .rounded(px(theme::RADIUS_MD))
        .cursor_pointer()
        // 这版 gpui 没有 `transform`，「上浮」只能用阴影 + 底色提亮（见 `theme::hover_shadow`）
        .hover(|style| style.bg(theme::hover()).shadow(theme::hover_shadow()))
        .child(
            div()
                .relative()
                .flex_none()
                .w_full()
                .child(cover)
                .child(super::card_play_button(
                    ("card-play", track_id.0),
                    group.clone(),
                    track_id,
                    library,
                    &title_for_play,
                    card.kind,
                )),
        )
        .child(
            // `w_full` + `min_w_0` + `truncate` 缺一不可：flex 子项默认 `min-width: auto`，
            // 会按文字固有宽度撑开整格，`truncate` 自己不会停止索取宽度
            div()
                .w_full()
                .min_w_0()
                .text_size(px(theme::FONT_BODY))
                .font_weight(theme::WEIGHT_BUTTON)
                .text_color(theme::text())
                .truncate()
                .group_hover(group, |style| style.text_color(theme::text()))
                .child(card.title.clone()),
        )
        .child(card_meta(card))
        .on_click({
            let grid = grid.clone();
            let event = event.clone();
            move |_, _, cx| {
                grid.update(cx, |_, cx| cx.emit(event.clone())).ok();
            }
        })
        .context_menu(crate::common::menu::group_context_menu(
            library.clone(),
            group_kind(card.kind),
            track_id,
            card.title.clone(),
            Rc::new({
                let grid = grid.clone();
                let event = event.clone();
                move |cx: &mut App| {
                    grid.update(cx, |_, cx| cx.emit(event.clone())).ok();
                }
            }),
        ))
}

/// 分区 → 菜单认的分组种类。`Tracks` 分支不可达，兜底成专辑只是给出一个明确行为。
fn group_kind(section: NavSection) -> crate::common::menu::GroupKind {
    match section {
        NavSection::Artists => crate::common::menu::GroupKind::Artist,
        _ => crate::common::menu::GroupKind::Album,
    }
}

fn card_meta(card: &Card) -> impl IntoElement {
    let mut meta = div().w_full().min_w_0().flex_none().flex_col().gap(px(1.));
    if !card.subtitle.is_empty() {
        meta = meta.child(
            div()
                .w_full()
                .min_w_0()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_muted())
                .truncate()
                .child(card.subtitle.clone()),
        );
    }
    meta.child(
        div()
            .w_full()
            .min_w_0()
            .text_size(px(theme::FONT_MICRO))
            .text_color(theme::text_faint())
            .truncate()
            .child(card.meta.clone()),
    )
}

/// 封面区。未命中缓存时向代表曲目发一次请求；全组没有封面时 `Slot::Missing` 终止重试。
fn cover_cell(
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    card: &Card,
    height: f32,
    cx: &mut App,
) -> impl IntoElement {
    let id = card.id;
    // 取图按最宽那一档请求：这一格是 `w_full`，高度却被 `cover_height` 夹在 132 以上，
    // 窄列时宽 > 高，按高度要图会横向放大
    let state = crate::common::cover::cover_state(covers, library, id, CARD_MAX_W, cx);

    let cell = div()
        .flex_none()
        .h(px(height))
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::elevated());

    match state {
        crate::common::cover::Cover::Ready(image) => {
            // `ObjectFit::Cover` 会把圆角裁到可见区域，gpui 也没有 `overflow_hidden`，
            // 所以这里不需要容器裁剪
            cell.child(
                img(ImageSource::Render(image))
                    .size_full()
                    .rounded(px(theme::RADIUS_LG))
                    .object_fit(ObjectFit::Cover),
            )
        }
        crate::common::cover::Cover::Missing => cell
            .rounded(px(theme::RADIUS_LG))
            .border_1()
            .border_color(theme::border())
            .child(monogram(card.initial)),
        // 还在解码：画骨架屏，别让首字色块闪一下再被真封面顶掉
        crate::common::cover::Cover::Pending => {
            cell.child(crate::common::cover::skeleton(height, theme::RADIUS_LG))
        }
    }
}

/// 没有封面时的占位：标题首字 + 按字符标量值取模选出的色块，同一首字颜色恒定。
fn monogram(initial: Option<char>) -> impl IntoElement {
    let Some(ch) = initial else {
        return div()
            .text_color(theme::text_faint())
            .child(icon_inherit(Glyph::Album, 32.));
    };

    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(crate::common::cover::monogram_color(ch))
        .text_size(px(32.))
        .text_color(rgb(0xffffff))
        .child(ch.to_string())
}

fn empty_grid() -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_start()
        .pt(px(theme::GAP_XL))
        .px(px(theme::GAP_LG))
        .gap(px(theme::GAP_MD))
        .child(
            div()
                .text_size(px(theme::FONT_DISPLAY))
                .font_weight(theme::WEIGHT_TITLE)
                .text_color(theme::text())
                .child("还没有可以分组的曲目"),
        )
        .child(
            div()
                .max_w(px(460.))
                .text_size(px(theme::FONT_BODY))
                .text_color(theme::text_muted())
                .child("导入一些音乐后，这里会按专辑和艺术家自动整理成封面网格。"),
        )
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`crate::platform::*` 会把 gpui 的 `test` 过程宏带进来，
    // 遮蔽内置 `#[test]`，报递归上限
    use super::{COVER_H_MAX, COVER_H_MIN, cover_height, total_items};
    use crate::shell::{CARD_MAX_W, CARD_MIN_W, ShellEntity};

    fn row_of(n: usize) -> Vec<super::Card> {
        (0..n)
            .map(|i| super::Card {
                id: lyre_core::TrackId(i as u64),
                title: format!("t{i}"),
                subtitle: String::new(),
                meta: String::new(),
                initial: None,
                kind: crate::shell::NavSection::Albums,
            })
            .collect()
    }

    #[test]
    fn header_counts_items_not_rows() {
        let rows = vec![row_of(6), row_of(2)];
        assert_eq!(rows.len(), 2);
        assert_eq!(total_items(&rows), 8);
    }

    #[test]
    fn empty_grid_counts_zero_items() {
        assert_eq!(total_items(&[]), 0);
        assert_eq!(total_items(&[row_of(0)]), 0);
    }

    /// 判错的后果是艺术家卡上的菜单写着「打开专辑」。
    #[test]
    fn a_card_maps_to_the_same_group_kind_the_menu_expects() {
        use super::group_kind;
        use crate::common::menu::GroupKind;
        use crate::shell::NavSection;

        assert_eq!(group_kind(NavSection::Artists), GroupKind::Artist);
        assert_eq!(group_kind(NavSection::Albums), GroupKind::Album);
        assert_eq!(group_kind(NavSection::Tracks), GroupKind::Album);
    }

    /// 真实卡宽 = `min(预测宽, CARD_MAX_W)`：`card_item` 上的 `max_w` 会截断布局层的预测值。
    fn real_card_width(predicted: f32) -> f32 {
        predicted.min(CARD_MAX_W)
    }

    fn aspect_ratio(predicted_w: f32) -> f32 {
        let w = real_card_width(predicted_w);
        if w <= 0.0 {
            return 1.0;
        }
        w / cover_height(w)
    }

    #[test]
    fn cover_never_becomes_a_landscape_banner() {
        // 范围刻意超过 `CARD_MAX_W`，那段正是 `max_w` 生效处
        let mut w = CARD_MIN_W - 40.0;
        while w <= CARD_MAX_W + 400.0 {
            let ratio = aspect_ratio(w);
            assert!(
                ratio <= 1.26,
                "预测宽 {w:.0}px（实际 {}px）时封面宽高比 {ratio:.2}，超过 1.26 就成了横幅",
                real_card_width(w)
            );
            w += 1.0;
        }
    }

    #[test]
    fn cover_is_never_stretched_beyond_square() {
        // 下界取 `CARD_MIN_W`：布局层保证卡宽 ≥ 它，更窄只出现在尚未完成布局的时刻
        let mut w = CARD_MIN_W;
        while w <= CARD_MAX_W {
            let ratio = aspect_ratio(w);
            assert!(
                ratio >= 0.76,
                "卡宽 {w:.0}px 时封面宽高比 {ratio:.2}，低于 0.76 就成了竖条"
            );
            w += 1.0;
        }
    }

    #[test]
    fn widest_card_gets_a_square_cover() {
        assert_eq!(cover_height(CARD_MAX_W), COVER_H_MAX);
        assert!((CARD_MAX_W - COVER_H_MAX).abs() < f32::EPSILON);
    }

    #[test]
    fn narrow_card_gets_the_minimum_cover_height() {
        assert_eq!(cover_height(CARD_MIN_W), COVER_H_MIN);
        assert_eq!(cover_height(0.0), COVER_H_MIN, "宽度不可用时退回下限");
    }

    #[test]
    fn cover_height_never_collapses_to_zero() {
        for w in [0.0, 1.0, 10.0, 50.0] {
            assert!(
                cover_height(w) >= COVER_H_MIN,
                "卡宽 {w}px 时封面高度塌成了 {}",
                cover_height(w)
            );
        }
    }

    #[test]
    fn the_max_width_actually_engages_on_ultrawide_screens() {
        let wide = [2560.0, 3440.0, 3840.0, 5120.0];
        let mut engaged = 0;
        for width in wide {
            let (_, predicted) = ShellEntity::grid_metrics_for(width);
            if predicted > CARD_MAX_W {
                engaged += 1;
            }
        }
        assert_eq!(
            engaged,
            wide.len(),
            "超宽屏上预测卡宽都没超过上限，max_w 永远轮不到，\
             而它正是防止横幅的最后一道闸"
        );
    }

    #[test]
    fn layout_layer_and_cover_height_agree_on_every_width() {
        // `ShellEntity` 预测的卡宽与 `cover_height` 的输入必须是同一个量
        for width in [560.0, 900.0, 1180.0, 1600.0, 2560.0] {
            let (columns, card_w) = ShellEntity::grid_metrics_for(width);
            assert!(columns >= 1, "宽 {width}px 得到 {columns} 列");
            assert!(card_w >= 0.0, "宽 {width}px 预测出负卡宽 {card_w}");
            if card_w > 0.0 {
                let h = cover_height(card_w);
                assert!(
                    (COVER_H_MIN..=COVER_H_MAX).contains(&h),
                    "宽 {width}px 卡宽 {card_w:.0} 推出封面高 {h}，越界"
                );
            }
        }
    }

    /// Windows 上真实造出来的文件名：中英混排、带冒号带括号，最容易在测量阶段漏算宽度。
    const REPRO_TITLES: [&str; 4] = [
        "Deluxe Expanded Remastered Collection Volume Seven",
        "A",
        "夜曲与城市漫游: 完整版录音室专辑精选集 (Deluxe Edition)",
        "The Very Long Artist Name That Should Never Stretch A Grid Card Beyond Reason",
    ];

    #[test]
    fn long_titles_cannot_influence_card_width() {
        // 卡宽是可用宽度的纯函数，与标题长度无关
        let longest = REPRO_TITLES
            .iter()
            .max_by_key(|t| t.chars().count())
            .expect("复现数据非空");
        let shortest = REPRO_TITLES
            .iter()
            .min_by_key(|t| t.chars().count())
            .expect("复现数据非空");
        assert!(
            longest.chars().count() > shortest.chars().count() * 10,
            "复现数据本身长度差异不够大，测不出文本对宽度的影响：\
             最短 {:?}({} 字符) vs 最长 {:?}({} 字符)",
            shortest,
            shortest.chars().count(),
            longest,
            longest.chars().count()
        );

        let (base_columns, base_card) = ShellEntity::grid_metrics_for(1920.0);
        for title in REPRO_TITLES {
            // 文本测量产出的是绘制宽度，不进布局公式
            let intrinsic = title.chars().count() as f32 * 8.0; // 粗略字宽
            let _drawn = intrinsic.min(CARD_MAX_W);
            let (columns, predicted) = ShellEntity::grid_metrics_for(1920.0);
            assert_eq!(
                (columns, predicted),
                (base_columns, base_card),
                "标题 {:?}({} 字符) 改变了布局结果：\
                 期望 ({}, {:.1})，实得 ({}, {:.1})",
                title,
                title.chars().count(),
                base_columns,
                base_card,
                columns,
                predicted
            );
        }

        for width in [560.0, 860.0, 1180.0, 1920.0, 3440.0] {
            let (columns, predicted) = ShellEntity::grid_metrics_for(width);
            let real = real_card_width(predicted);

            assert!(
                real <= CARD_MAX_W + f32::EPSILON,
                "宽 {width}px 下卡宽 {real} 超过上限 {CARD_MAX_W}"
            );

            let ratio = if real <= 0.0 {
                1.0
            } else {
                real / cover_height(real)
            };
            assert!(
                ratio <= 1.26,
                "宽 {width}px、含 {} 字符标题时封面比例 {ratio:.2}，已成横幅",
                longest.chars().count()
            );

            // 同一个可用宽度重复计算必须得到同一结果：列数与卡宽同源
            let (columns_again, predicted_again) = ShellEntity::grid_metrics_for(width);
            assert_eq!(
                (columns, predicted),
                (columns_again, predicted_again),
                "宽 {width}px 重复计算得到不同结果，列数与卡宽没有同源"
            );
            assert!(
                (1..=crate::shell::MAX_GRID_COLUMNS).contains(&columns),
                "宽 {width}px 得到 {columns} 列，越出 1..={}",
                crate::shell::MAX_GRID_COLUMNS
            );
        }
    }
}
