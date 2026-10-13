//! 横向滚动区块，以及区块里的三种卡（专辑海报卡 / 歌单卡 / 最近播放卡）。
//! `overflow_x_scroll` 的偏移存在帧状态里没有 getter，箭头滚动只能走 [`ScrollHandle`]。
//! 不挂 `.track_scroll()` 时 `bounds_for_item` 恒为 `None`，步长就没法算。

use lyre_core::TrackId;

use crate::common::button::IconButton;
use crate::components::detail::HeroInfo;
use crate::cover::CoverCache;
use crate::entities::{LibraryEntity, Scope};
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::shell::{NavSection, ShellEntity};
use crate::theme;

use super::HomeView;
use super::cards::RecentCard;

const RECENT_COVER: f32 = 140.0;
const RECENT_PAD: f32 = theme::GAP_SM;
/// 卡宽必须把左右内边距算进来：只取封面边长会让内容溢出 hover 底色一圈。
pub(super) const RECENT_W: f32 = RECENT_COVER + RECENT_PAD * 2.0;

/// 快捷网格一行放几张：由可用宽度反推，不写死。
pub(super) fn grid_columns_in(usable: f32) -> usize {
    let gap = theme::GAP_MD;
    ((usable + gap) / (RECENT_W + gap)).floor().max(1.0) as usize
}
/// 总高 = 上下内边距 + 封面 + 间距 + 标题两行。
const RECENT_H: f32 = RECENT_PAD * 2.0 + RECENT_COVER + 8.0 + 36.0;
/// 横向滚动区块里每张海报的封面边长（整卡宽 = 它 + 左右内边距）。
const POSTER_COVER: f32 = 160.0;
const POSTER_PAD: f32 = theme::GAP_MD;

impl HomeView {
    /// 取（必要时新建）某个横向区块的滚动句柄；每帧新建一个等于把偏移归零。
    fn row_handle(&self, id: &'static str) -> ScrollHandle {
        self.rows.borrow_mut().entry(id).or_default().clone()
    }

    /// 箭头的一格 = 一张卡的宽 + 卡间距，用实测边界而不是按断点算。
    /// 不必自己夹边界：`overflow` 那一趟会把偏移夹回 `[-max, 0]` 并写回同一个 `RefCell`。
    fn nudge_row(handle: &ScrollHandle, forward: bool, cx: &mut Context<Self>) {
        let step = handle
            .bounds_for_item(0)
            .map_or(200.0, |bounds| f32::from(bounds.size.width) + theme::GAP_MD);
        let current = f32::from(handle.offset().x);
        let next = if forward {
            current - step
        } else {
            current + step
        };
        handle.set_offset(point(px(next), px(0.)));
        cx.notify();
    }
}

/// 「正好到头」的容差。`max_offset` 按 2 位小数取整，加上亚像素滚动，差零点几像素是常态；
/// 不留容差的话，差 0.3px 就判成「没到头」，右箭头永远灰不下去。
const EDGE_EPS: f32 = 0.5;

/// 一排的两端状态，箭头据此显隐。首帧（还没布局过）两个方向都算能滚。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct RowEdges {
    at_start: bool,
    at_end: bool,
}

impl RowEdges {
    /// 由句柄读到的三个数推出来。`viewport_w <= 0` 就是「还没量过」，此时返回「两端都没到」。
    pub(super) fn of(viewport_w: f32, offset_x: f32, max_offset_x: f32) -> Self {
        if viewport_w <= 0.0 {
            return Self {
                at_start: false,
                at_end: false,
            };
        }
        Self {
            at_start: offset_x >= -EDGE_EPS,
            // 内容装得下时 `max_offset_x` 是 0，此时两个方向同时到头。
            at_end: offset_x <= -max_offset_x + EDGE_EPS,
        }
    }

    /// 这个方向还能不能滚。未测量时两个方向都算能。
    fn can_go(self, forward: bool) -> bool {
        if forward {
            !self.at_end
        } else {
            !self.at_start
        }
    }
}

pub(super) fn section_title(text: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .pb(px(theme::GAP_MD))
        .child(section_heading(text))
}

/// 区块标题的字身。下边距交给带箭头的标题行那一整行，否则同一排里标题和箭头高度不一致。
fn section_heading(text: &'static str) -> impl IntoElement {
    div()
        .text_size(px(theme::FONT_TITLE))
        .font_weight(theme::WEIGHT_TITLE)
        .text_color(theme::text())
        .child(text)
}

/// 一张最近播放卡片：封面 + 专辑名 + 艺术家，点击即从这首开始播；Hover 时封面右下角浮现播放键。
pub(super) fn recent_card(
    index: usize,
    card: RecentCard,
    library: &Entity<LibraryEntity>,
    covers: &Entity<CoverCache>,
    cx: &mut App,
) -> AnyElement {
    let id = card.id;
    let target = library.clone();
    let play_library = library.clone();
    let group = SharedString::from(format!("home-recent-g-{index}"));
    let cover = cover_slot(covers, library, id, RECENT_COVER, &card.album, cx);
    let play_title = card.album.clone();
    // 点这张卡 = 播这张专辑，所以队列按专辑名定；`card.album` 还要给标题用，各自 clone
    let queue_album = card.album.clone();
    let subtitle = if card.artist.is_empty() {
        card.meta.clone()
    } else {
        format!("{} · {}", card.artist, card.meta)
    };

    div()
        .id(SharedString::from(format!("home-recent-{index}")))
        .group(group.clone())
        .flex_none()
        .w(px(RECENT_W))
        .h(px(RECENT_H))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .rounded(px(theme::RADIUS_CARD))
        .p(px(RECENT_PAD))
        .cursor_pointer()
        .hover(|style| style.bg(theme::elevated()))
        .on_click(move |_, _, cx| {
            // `activate` 只发事件，装载播放器由根视图做。
            target.update(cx, |library, cx| {
                library.activate_album(&queue_album, id, cx);
            });
        })
        .child(div().relative().flex_none().w_full().child(cover).child(
            crate::components::card_play_button(
                SharedString::from(format!("home-recent-play-{index}")),
                group,
                id,
                &play_library,
                &play_title,
                // 首页三处横排的卡都是专辑卡
                NavSection::Albums,
            ),
        ))
        .child(
            div()
                .flex_none()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(theme::FONT_BODY))
                        .font_weight(theme::WEIGHT_SUBTITLE)
                        .text_color(theme::text())
                        .truncate()
                        .child(card.album),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(theme::text_muted())
                        .truncate()
                        .child(subtitle),
                ),
        )
        .into_any_element()
}

/// 一个横向滚动区块的外壳：标题（带左右箭头）+ 一排可横向滚动的卡，卡片由调用方造好传进来。
fn scroll_row(
    id: &'static str,
    title: &'static str,
    cards: Vec<AnyElement>,
    home: &HomeView,
    view: Entity<HomeView>,
) -> AnyElement {
    let handle = home.row_handle(id);
    div()
        .flex_none()
        .flex()
        .flex_col()
        .child(section_header(title, id, handle.clone(), view))
        .child(
            div()
                .id(id)
                .flex_none()
                .flex()
                .gap(px(theme::GAP_MD))
                .pb(px(theme::GAP_XL))
                .track_scroll(&handle)
                .overflow_x_scroll()
                .children(cards),
        )
        .into_any_element()
}

/// 标题那一行：字在左，一对箭头在右。到头的那个方向变灰、不绑事件、也不进 Tab 序列。
fn section_header(
    title: &'static str,
    id: &'static str,
    handle: ScrollHandle,
    view: Entity<HomeView>,
) -> AnyElement {
    let edges = RowEdges::of(
        f32::from(handle.bounds().size.width),
        f32::from(handle.offset().x),
        f32::from(handle.max_offset().x),
    );

    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .pb(px(theme::GAP_MD))
        .child(section_heading(title))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_XS))
                .child(row_arrow(id, false, handle.clone(), view.clone(), edges))
                .child(row_arrow(id, true, handle, view, edges)),
        )
        .into_any_element()
}

/// 区块标题右侧的一枚箭头。gpui 没有 `transform`，镜像 `ChevronRight` 走不通，所以用一对方向图标。
fn row_arrow(
    id: &'static str,
    forward: bool,
    handle: ScrollHandle,
    view: Entity<HomeView>,
    edges: RowEdges,
) -> AnyElement {
    let glyph = if forward {
        Glyph::ArrowRight
    } else {
        Glyph::ArrowLeft
    };
    let enabled = edges.can_go(forward);
    let key = SharedString::from(format!(
        "{id}-arrow-{}",
        if forward { "next" } else { "prev" }
    ));

    // tooltip 同时是无障碍名称：禁用态也要说清为什么点不动。
    let label = match (enabled, forward) {
        (true, true) => "下一批",
        (true, false) => "上一批",
        (false, true) => "已经是最后一批",
        (false, false) => "已经是第一批",
    };

    IconButton::new(key, glyph)
        .box_size(26.0)
        .glyph_size(13.0)
        .label(label)
        .enabled(enabled)
        .on_click(move |_, _, cx| {
            view.update(cx, |_home, cx| HomeView::nudge_row(&handle, forward, cx));
        })
        .render()
        .into_any_element()
}

/// 一个横向滚动区块：一排专辑海报卡。滚动交给 gpui 的 `overflow_x_scroll`，不自建滚动容器。
pub(super) fn horizontal_section(
    id: &'static str,
    title: &'static str,
    cards: Vec<RecentCard>,
    home: &HomeView,
    view: Entity<HomeView>,
    cx: &mut App,
) -> AnyElement {
    let posters = cards
        .into_iter()
        .enumerate()
        .map(|(i, card)| poster_card(id, i, card, &home.library, &home.covers, cx))
        .collect();
    scroll_row(id, title, posters, home, view)
}

/// 横向区块里的海报卡（竖向：封面在上、两行文字在下）。
fn poster_card(
    section: &'static str,
    index: usize,
    card: RecentCard,
    library: &Entity<LibraryEntity>,
    covers: &Entity<CoverCache>,
    cx: &mut App,
) -> AnyElement {
    let id = card.id;
    let target = library.clone();
    let play_library = library.clone();
    let group = SharedString::from(format!("{section}-g-{index}"));
    let cover = cover_slot(covers, library, id, POSTER_COVER, &card.album, cx);
    let play_title = card.album.clone();
    let queue_album = card.album.clone();
    let subtitle = if card.artist.is_empty() {
        card.meta.clone()
    } else {
        format!("{} · {}", card.artist, card.meta)
    };

    div()
        .id((section, index))
        .group(group.clone())
        .flex_none()
        .w(px(POSTER_COVER + POSTER_PAD * 2.0))
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .p(px(POSTER_PAD))
        .rounded(px(theme::RADIUS_MD))
        .cursor_pointer()
        .hover(|style| style.bg(theme::hover()))
        .on_click(move |_, _, cx| {
            target.update(cx, |library, cx| {
                library.activate_album(&queue_album, id, cx);
            });
        })
        .child(div().relative().flex_none().w_full().child(cover).child(
            crate::components::card_play_button(
                SharedString::from(format!("{section}-play-{index}")),
                group,
                id,
                &play_library,
                &play_title,
                NavSection::Albums,
            ),
        ))
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_size(px(theme::FONT_BODY))
                .font_weight(theme::WEIGHT_SUBTITLE)
                .text_color(theme::text())
                .truncate()
                .child(card.album),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_muted())
                .truncate()
                .child(subtitle),
        )
        .into_any_element()
}

/// 一排歌单卡：收藏 + 每个自建歌单。
pub(super) fn list_section(
    id: &'static str,
    title: &'static str,
    cards: Vec<(Scope, HeroInfo)>,
    home: &HomeView,
    view: Entity<HomeView>,
    cx: &mut App,
) -> AnyElement {
    let row = cards
        .into_iter()
        .enumerate()
        .map(|(index, (scope, info))| {
            list_card(
                index,
                scope,
                info,
                &home.library,
                &home.covers,
                &home.shell,
                cx,
            )
        })
        .collect();
    scroll_row(id, title, row, home, view)
}

/// 一张歌单卡。与专辑海报卡同尺寸，但点它是收窄作用域并切到曲库页，不是开播，所以不画 hover 播放键。
fn list_card(
    index: usize,
    scope: Scope,
    info: HeroInfo,
    library: &Entity<LibraryEntity>,
    covers: &Entity<CoverCache>,
    shell: &Entity<ShellEntity>,
    cx: &mut App,
) -> AnyElement {
    let cover = crate::common::cover::list_cover(
        covers,
        library,
        POSTER_COVER,
        theme::RADIUS_LG,
        info.cover_file.as_deref(),
        info.cover_id,
        &info.title,
        cx,
    );
    let shell = shell.clone();
    let library = library.clone();

    div()
        .id(("home-list", index))
        .flex_none()
        .w(px(POSTER_COVER + POSTER_PAD * 2.0))
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .p(px(POSTER_PAD))
        .rounded(px(theme::RADIUS_MD))
        .cursor_pointer()
        .hover(|style| style.bg(theme::hover()))
        .on_click(move |_, _, cx| {
            crate::shell::enter_scope_from_here(scope.clone(), &shell, &library, cx);
        })
        .child(cover)
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_size(px(theme::FONT_BODY))
                .font_weight(theme::WEIGHT_SUBTITLE)
                .text_color(theme::text())
                .truncate()
                .child(info.title),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_muted())
                .truncate()
                .child(info.meta),
        )
        .into_any_element()
}

/// 封面槽：命中就显示图片，否则发一次请求并显示占位。
fn cover_slot(
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    id: TrackId,
    size: f32,
    label: &str,
    cx: &mut App,
) -> AnyElement {
    let state = crate::common::cover::cover_state(covers, library, id, size, cx);

    let cell = div()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_LG))
        .bg(theme::elevated());

    match state {
        crate::common::cover::Cover::Ready(image) => cell
            .child(
                img(ImageSource::Render(image))
                    .size_full()
                    .rounded(px(theme::RADIUS_LG))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element(),
        // 确认没有封面：首字色块。
        crate::common::cover::Cover::Missing => cell
            .border_1()
            .border_color(theme::border())
            .child(monogram(label.chars().next()))
            .into_any_element(),
        // 解码中：骨架屏。
        crate::common::cover::Cover::Pending => cell
            .child(crate::common::cover::skeleton(size, theme::RADIUS_LG))
            .into_any_element(),
    }
}

/// 没有封面时的占位：标题首字 + 稳定色块。
/// 调色板必须与 `album_grid::monogram` 一致 —— 同一专辑在两处颜色不同会被当成两张专辑。
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
        .rounded(px(theme::RADIUS_LG))
        .bg(crate::common::cover::monogram_color(ch))
        .text_color(rgb(0xffffff))
        .text_size(px(32.0))
        .child(ch.to_string())
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`platform::*` 里的 gpui `test` 过程宏会遮蔽内置 `#[test]`。
    use super::RowEdges;

    /// 首帧放行：布局还没跑过时读到的 `bounds` / `max_offset` 都是 0，照它断言「到底」会让两枚箭头一起灭掉。
    #[test]
    fn the_first_frame_claims_nothing() {
        let unmeasured = RowEdges::of(0.0, 0.0, 0.0);
        assert!(unmeasured.can_go(false), "未测量时左箭头要可用");
        assert!(unmeasured.can_go(true), "未测量时右箭头要可用");
    }

    /// 一屏装得下：`max_offset` 为 0，两个方向同时到头 —— 两枚都该灰。
    #[test]
    fn a_row_that_fits_disables_both_arrows() {
        let edges = RowEdges::of(600.0, 0.0, 0.0);
        assert!(!edges.can_go(false) && !edges.can_go(true));
    }

    #[test]
    fn each_arrow_dies_only_at_its_own_end() {
        let at_left = RowEdges::of(600.0, 0.0, 900.0);
        assert!(!at_left.can_go(false), "在最左边，左箭头该灰");
        assert!(at_left.can_go(true), "在最左边，右箭头该亮");

        let at_right = RowEdges::of(600.0, -900.0, 900.0);
        assert!(at_right.can_go(false), "在最右边，左箭头该亮");
        assert!(!at_right.can_go(true), "在最右边，右箭头该灰");
    }

    /// 容差两侧都要验：太宽会让箭头永远灰不下去，太紧会在还有卡片时就灰掉。
    #[test]
    fn the_end_tolerance_is_half_a_pixel() {
        // `max_offset` 按 2 位小数取整，加上亚像素滚动，正好到头也能差零点几像素。
        assert!(!RowEdges::of(600.0, -899.7, 900.0).can_go(true));
        // 差得多就还是「没到头」。
        assert!(RowEdges::of(600.0, -880.0, 900.0).can_go(true));
        assert!(!RowEdges::of(600.0, -0.3, 900.0).can_go(false));
        assert!(RowEdges::of(600.0, -8.0, 900.0).can_go(false));
    }
}
