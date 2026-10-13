//! 侧边栏的两张卡片与卡片内部的小件；图标态下整栏由 [`Sidebar::render_icons`] 画。

use std::rc::Rc;

use crate::common::{thousands, tooltip};
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::shell::{NavSection, SidebarState};
use crate::theme;

use super::Sidebar;
use super::entries::{Listing, SideEntry, lib_row};
use super::nav::{NavDeps, home_item, lib_fallback_item, library_item, library_item_selected};

const CARD_PAD: f32 = theme::GAP_SM;
const LIB_HEADER_H: f32 = 32.0;
/// 下卡片虚拟列表最多渲染的行数，防御性截断。
const LIB_MAX_ROWS: usize = 1000;

impl Sidebar {
    /// 上卡片：首页 / 曲库两项，定高不参与滚动。没有「搜索」项 —— 搜索只由顶栏那个框进入。
    pub(super) fn home_card(&self, state: SidebarState, cx: &mut App) -> AnyElement {
        let labels = state.shows_labels();
        let nav = self.shell.read(cx).nav_section();
        let scoped = self.library.read(cx).scope().is_scoped();
        let card = div()
            .flex_none()
            .flex()
            .flex_col()
            .rounded(px(theme::RADIUS_CARD))
            .overflow_hidden()
            .bg(theme::panel())
            .p(px(CARD_PAD))
            .gap(px(2.));

        let deps = || NavDeps {
            shell: self.shell.clone(),
            library: self.library.clone(),
        };

        card.child(home_item(
            "side-home",
            "首页",
            labels,
            nav == NavSection::Home,
            deps(),
        ))
        .child(library_item(
            "side-library",
            "曲库",
            labels,
            library_item_selected(nav, scoped),
            deps(),
        ))
        .into_any_element()
    }

    /// 「你的音乐库」卡片：标题行 + Chip 筛选 + 专辑 / 艺术家列表。
    pub(super) fn library_card(&self, state: SidebarState, cx: &mut Context<Self>) -> AnyElement {
        let labels = state.shows_labels();
        let nav = self.shell.read(cx).nav_section();
        let scoped = self.library.read(cx).scope().is_scoped();
        let effective = self.effective_listing(nav);

        let entries = self.entries(nav, cx);

        let header = lib_header(labels, &self.library);

        if !labels {
            // 图标态放不下带缩略图的行，退回到单个「库」图标。
            return card_shell(false)
                .child(header)
                .child(lib_fallback_item(
                    scoped,
                    NavDeps {
                        shell: self.shell.clone(),
                        library: self.library.clone(),
                    },
                ))
                .into_any_element();
        }

        let chips = Self::chips_row(effective, cx);

        if entries.is_empty() {
            return card_shell(true)
                .child(header)
                .child(chips)
                .child(empty_hint(self.library.clone()))
                .into_any_element();
        }

        // 行数上限是防御性的：`item_count` 过大时 `uniform_list` 内部总高溢出成负数，列表一进来就滚到底。
        let rows: Rc<Vec<SideEntry>> = Rc::new(entries);
        let row_count = rows.len().min(LIB_MAX_ROWS);

        // `min_h_0` 不能省：少了它 `uniform_list` 的最小高度会被内容撑到全高，卡片滚不动。
        let list = uniform_list("side-library-list", row_count, {
            let rows = Rc::clone(&rows);
            let shell = self.shell.clone();
            let library = self.library.clone();
            let covers = self.covers.clone();
            move |range, _window, cx| {
                range
                    .map(|ix| {
                        lib_row(
                            rows[ix].clone(),
                            shell.clone(),
                            library.clone(),
                            &covers,
                            cx,
                        )
                    })
                    .collect()
            }
        })
        .flex_1()
        .min_h_0()
        .into_any_element();

        card_shell(true)
            .child(header)
            .child(chips)
            .child(list)
            .into_any_element()
    }

    /// 「你的音乐库」标题下的胶囊 Chip 行：手动切换下卡片列歌单、艺术家还是专辑。
    fn chips_row(effective: Listing, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("side-lib-chips")
            .flex_none()
            .flex()
            .gap(px(theme::GAP_SM))
            .px(px(theme::GAP_SM))
            .pb(px(theme::GAP_SM))
            .child(chip(
                "side-chip-collections",
                "歌单",
                effective == Listing::Collections,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.set_listing(Listing::Collections, cx);
                }),
            ))
            .child(chip(
                "side-chip-artists",
                "艺术家",
                effective == Listing::Artists,
                cx.listener(|this, _: &ClickEvent, _, cx| this.set_listing(Listing::Artists, cx)),
            ))
            .child(chip(
                "side-chip-albums",
                "专辑",
                effective == Listing::Albums,
                cx.listener(|this, _: &ClickEvent, _, cx| this.set_listing(Listing::Albums, cx)),
            ))
            .into_any_element()
    }

    /// 图标态（56px）的整栏渲染。
    pub(super) fn render_icons(
        &self,
        state: SidebarState,
        counts: (usize, usize, usize),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let nav = self.shell.read(cx).nav_section();
        let scoped = self.library.read(cx).scope().is_scoped();

        let mut card = div()
            .flex_none()
            .w(px(state.width()))
            .h_full()
            .flex()
            .flex_col()
            .rounded(px(theme::RADIUS_CARD))
            .overflow_hidden()
            .bg(theme::panel())
            .p(px(CARD_PAD))
            .gap(px(2.));

        let deps = || NavDeps {
            shell: self.shell.clone(),
            library: self.library.clone(),
        };

        card = card.child(home_item(
            "side-home-icon",
            "首页",
            false,
            nav == NavSection::Home,
            deps(),
        ));

        let (tracks, albums, artists) = counts;
        card = card.child(
            div()
                .flex_none()
                .my(px(theme::GAP_XS))
                .h(px(1.))
                .mx(px(theme::GAP_SM))
                .bg(theme::border_soft()),
        );
        card = card.child(lib_fallback_item(scoped, deps()));

        let label = format!(
            "音乐库 · {} 首 / {} 张专辑 / {} 位艺术家",
            thousands(tracks),
            thousands(albums),
            thousands(artists)
        );
        card.id("side-icons")
            .tooltip(tooltip::builder(label))
            .into_any_element()
    }
}

/// 下卡片外壳。`fills` = true 吃掉剩余高度（展开态列表要滚），false 按内容定高（图标态）。
fn card_shell(fills: bool) -> Div {
    let card = div()
        .flex()
        .flex_col()
        .min_h_0()
        .rounded(px(theme::RADIUS_CARD))
        // 不裁剪的话内部铺满的背景会把圆角盖成直角。
        .overflow_hidden()
        .bg(theme::panel())
        .p(px(CARD_PAD));

    if fills {
        card.flex_1()
    } else {
        card.flex_none()
    }
}

/// 下卡片标题行：分组标签 + 导入按钮，这一行本身不参与选中。
fn lib_header(labels: bool, library: &Entity<LibraryEntity>) -> AnyElement {
    let mut row = div()
        .flex_none()
        .h(px(LIB_HEADER_H))
        .flex()
        .items_center()
        .gap(px(theme::GAP_SM))
        .px(px(theme::GAP_SM));

    row = row.child(
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(px(24.))
            .text_color(theme::text_muted())
            .child(icon_inherit(Glyph::Folder, 20.)),
    );

    if labels {
        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .child(crate::common::section::section_heading("你的音乐库")),
        );

        row = row.child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(theme::GAP_MD))
                .child(header_action("side-lib-add", Glyph::Plus, "导入音乐", {
                    let library = library.clone();
                    move |_, _, cx| library.update(cx, LibraryEntity::open_dialog)
                })),
        );
    }

    row.into_any_element()
}

/// 标题行右侧的小圆按钮。
fn header_action(
    id: &'static str,
    glyph: Glyph,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex_none()
        .size(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .text_color(theme::text_muted())
        .hover(|style| style.bg(theme::hover()).text_color(theme::text()))
        .on_click(on_click)
        .child(icon_inherit(glyph, 16.))
        .tooltip(tooltip::builder(label))
        .into_any_element()
}

/// 「你的音乐库」标题下的筛选胶囊，配色走 [`chip_colors`]。
fn chip(
    id: &'static str,
    label: &'static str,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (base_bg, base_color) = crate::common::button::chip_colors(selected);

    div()
        .id(id)
        .flex_none()
        .h(px(28.))
        .px(px(theme::GAP_MD))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_PILL))
        .text_size(px(theme::FONT_CAPTION))
        .font_weight(theme::WEIGHT_BUTTON)
        .cursor_pointer()
        .bg(base_bg)
        .text_color(base_color)
        .when(!selected, |chip| {
            chip.hover(|style| style.bg(theme::border()).text_color(theme::text()))
        })
        .on_click(on_click)
        .child(label)
        .into_any_element()
}

/// 空曲库提示：一句说明 + 一个可点的导入入口。
fn empty_hint(library: Entity<LibraryEntity>) -> AnyElement {
    div()
        .id("side-lib-empty")
        .flex_col()
        .gap(px(theme::GAP_SM))
        .px(px(theme::GAP_SM))
        .py(px(theme::GAP_MD))
        .child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_faint())
                .child("还没有音乐"),
        )
        .child(
            div()
                .id("side-lib-empty-import")
                .cursor_pointer()
                .text_size(px(theme::FONT_CAPTION))
                .font_weight(theme::WEIGHT_BUTTON)
                .text_color(theme::text())
                .hover(|style| style.text_color(theme::control_hover()))
                .on_click(move |_, _, cx| library.update(cx, LibraryEntity::open_dialog))
                .child("导入音乐"),
        )
        .into_any_element()
}
