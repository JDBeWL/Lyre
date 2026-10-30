//! 页头工具条：大标题、扫描指示、排序、清空、多选开关。

use lyre_core::SortKey;

use crate::common::button::{ButtonKind, text_button};
use crate::common::thousands;
use crate::entities::{LibraryEntity, ScanProgress};
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::theme;

use super::{TrackList, TrackListEvent};

/// 工具条渲染需要的全部数值，一次从曲库读出。
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct ToolbarState {
    /// 可见行数（过滤生效时小于 `total`）
    pub(super) count: usize,
    /// 曲库总数
    pub(super) total: usize,
    /// 用户真的输入了搜索词，而不是「有没有投影」：见 [`LibraryEntity::has_query`]
    pub(super) filtering: bool,
    pub(super) sort_key: SortKey,
    pub(super) scanning: Option<ScanProgress>,
    /// 是否处于专辑 / 艺术家详情里，决定要不要给浮动返回按钮让出左内边距
    pub(super) scoped: bool,
    /// 详情页的标题（专辑名 / 艺术家名）。`None` 表示没有收窄作用域
    pub(super) scope: Option<String>,
    /// 上方已有页头（详情页头图 / 搜索页大标题）时不再画大标题；列头照常画
    pub(super) headed_elsewhere: bool,
    pub(super) in_search: bool,
    pub(super) selecting: bool,
}

pub(super) fn can_reorder(in_collection: bool, has_query: bool, rows: usize) -> bool {
    in_collection && !has_query && rows > 1
}

/// 页头大标题的文案：搜索词 > 作用域名 > 「曲库 · N 首」。
fn header_summary(filtering: bool, scope: Option<&str>, count: usize, total: usize) -> String {
    if filtering {
        format!("找到 {} / {} 首", thousands(count), thousands(total))
    } else if let Some(name) = scope.filter(|name| !name.trim().is_empty()) {
        name.to_owned()
    } else {
        format!("曲库 · {} 首", thousands(total))
    }
}

/// 「排序 / 清空」只在曲库列表页显示，详情页与搜索页都不画。
/// 判据是「停在搜索页」而不是「有搜索词」：曲库页用顶栏过滤时按钮留在原地。
fn shows_actions(scoped: bool, in_search: bool) -> bool {
    !scoped && !in_search
}

pub(super) fn toolbar(
    library: Entity<LibraryEntity>,
    list: WeakEntity<TrackList>,
    state: ToolbarState,
) -> impl IntoElement {
    let ToolbarState {
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
    } = state;

    // 进详情时让出左内边距，免得大标题被浮动返回按钮压住；与按钮位置同源于 `crate::root::HEADER_BACK_INSET`
    let back_inset = if scoped {
        crate::root::HEADER_BACK_INSET
    } else {
        theme::GAP_LG
    };

    // 详情里的上留白再叠一档返回按钮的高度，让大标题落在按钮下方
    let top_pad = if scoped {
        crate::root::HEADER_BACK_INSET + theme::GAP_XL
    } else {
        theme::GAP_XL
    };

    let summary = header_summary(filtering, scope.as_deref(), count, total);

    let mut bar = div()
        .flex_none()
        .flex()
        .items_end()
        .gap(px(theme::GAP_MD))
        .pt(px(if headed_elsewhere { 0.0 } else { top_pad }))
        .pb(px(theme::GAP_MD))
        .pl(px(back_inset))
        .pr(px(theme::GAP_LG))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(theme::GAP_MD))
                // 扫描指示与页头归属无关，`headed_elsewhere` 时仍要画
                .when(!headed_elsewhere, |row| {
                    row.child(
                        div()
                            // 可缩 + 截断，不能 `flex_none`：大字撑破外壳时墨迹会盖到右边那排按钮上
                            .flex_shrink_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(theme::FONT_HERO))
                            .font_weight(theme::WEIGHT_TITLE)
                            .text_color(theme::text())
                            .child(summary),
                    )
                })
                .children(scanning.map(scan_indicator)),
        );

    if shows_actions(scoped, in_search) {
        bar = bar.child(sort_menu(library.clone(), total, sort_key));

        if total > 0 {
            let list = list.clone();
            bar = bar.child(text_button(
                "header-clear",
                "清空",
                ButtonKind::Ghost,
                move |_, _, cx| {
                    let previous = library.update(cx, LibraryEntity::clear);
                    list.update(cx, |_, cx| cx.emit(TrackListEvent::Cleared(previous)))
                        .ok();
                },
            ));
        }
    }

    // 多选是全列表的功能（详情页、搜索页同样要批量收藏 / 批量进歌单），所以不进 `shows_actions`
    if count > 0 {
        bar = bar.child(text_button(
            "header-select",
            if selecting { "退出多选" } else { "多选" },
            if selecting {
                ButtonKind::Secondary
            } else {
                ButtonKind::Ghost
            },
            move |_, _, cx| {
                list.update(cx, TrackList::toggle_selection_mode).ok();
            },
        ));
    }

    bar
}

/// 扫描进度指示：不确定进度（只报已找到数），因为扫描前不知道总文件数，百分比只能是假的。
fn scan_indicator(progress: ScanProgress) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(theme::GAP_XS))
        .child(
            div()
                .flex_none()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::success())
                .child("正在扫描"),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_muted())
                .child(format!("已找到 {}", thousands(progress.found))),
        )
}

/// 排序入口。不用 `context_menu`：它只在右键打开，左键点下去毫无反应。
fn sort_menu(library: Entity<LibraryEntity>, total: usize, current: SortKey) -> AnyElement {
    if total == 0 {
        return div().into_any_element();
    }

    let trigger = crate::common::button::button_shell(
        "sort-menu",
        crate::common::button::ButtonKind::Outline,
    )
    .tooltip(crate::common::tooltip::builder("排序方式"))
    .child(icon_inherit(Glyph::Sort, 14.))
    .child(current.label());

    // 菜单项在闭包里现造：`PopupMenuItem` 不是 `Clone`（内含 `Box<dyn Fn>`），而 builder 会被反复调用。
    crate::common::menu::dropdown(trigger, Anchor::TopRight, move |menu, _window, _cx| {
        let library = library.clone();
        let mut menu = menu;
        for key in SortKey::ALL {
            let library = library.clone();
            let checked = if key == current { "✓ " } else { "" };
            menu = menu.item(
                PopupMenuItem::new(format!("{checked}{}", key.label())).on_click(
                    move |_, _, cx| {
                        library.update(cx, |library, cx| library.sort_by(key, cx));
                    },
                ),
            );
        }
        menu
    })
}

#[cfg(test)]
mod tests {
    use super::{can_reorder, header_summary, shows_actions};

    #[test]
    fn reorder_needs_a_playlist_a_still_list_and_something_to_move() {
        assert!(can_reorder(true, false, 2));
        assert!(!can_reorder(true, false, 1), "一行没什么可排的");
        assert!(!can_reorder(true, true, 20), "搜索筛出来的行不该拖");
        assert!(
            !can_reorder(false, false, 20),
            "曲库 / 专辑 / 艺术家的位置不由用户定"
        );
    }

    #[test]
    fn actions_only_belong_to_the_library_page() {
        assert!(!shows_actions(true, false), "详情页不该有排序 / 清空");
        assert!(!shows_actions(false, true), "搜索页不该有排序 / 清空");
        assert!(shows_actions(false, false), "曲库列表页要有排序 / 清空");
        assert!(!shows_actions(true, true), "详情页 + 搜索页也不该有");
    }

    #[test]
    fn detail_page_titles_the_thing_you_entered() {
        assert_eq!(
            header_summary(false, Some("Chill Studio"), 3, 24),
            "Chill Studio",
            "详情页的大标题就是专辑 / 艺术家的名字，不是搜索结果文案"
        );
    }

    #[test]
    fn plain_library_titles_the_whole_collection() {
        assert_eq!(header_summary(false, None, 24, 24), "曲库 · 24 首");
    }

    #[test]
    fn searching_wins_over_the_scope_it_happens_inside() {
        assert_eq!(
            header_summary(true, Some("Chill Studio"), 1, 24),
            "找到 1 / 24 首",
            "有搜索词时搜索的文案优先于作用域名"
        );
    }

    #[test]
    fn summary_never_renders_an_empty_title() {
        // 空标题会让页头那一行塌成空白
        for filtering in [true, false] {
            for scope in [None, Some(""), Some("某个名字")] {
                for (count, total) in [(0usize, 0usize), (0, 24), (3, 24), (9, 9)] {
                    let summary = header_summary(filtering, scope, count, total);
                    assert!(
                        !summary.trim().is_empty(),
                        "filtering={filtering} scope={scope:?} count={count} total={total} \
                         时标题成了空串"
                    );
                }
            }
        }
    }
}
