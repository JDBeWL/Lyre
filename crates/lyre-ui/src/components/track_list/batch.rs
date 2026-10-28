//! 多选模式下的批量操作条：出现在工具条与列头之间。

use std::collections::HashSet;
use std::rc::Rc;

use lyre_core::{Collection, CollectionId, TrackId};

use crate::common::button::{ButtonKind, text_button};
use crate::common::thousands;
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::theme;

use super::TrackList;
use super::selection::ordered_chosen;

pub(super) struct BatchBar {
    /// 已勾选的可见曲目数量；选择已在投影换代时清理过。
    pub(super) chosen_count: usize,
    /// 勾选集合，只有展开「加入歌单」菜单、需要逐项 ✓ 时才排序。
    pub(super) selected: Rc<HashSet<TrackId>>,
    /// 可见行数，「全选」的提示用
    pub(super) rows: usize,
    /// 当前正看着哪个歌单 —— 决定给不给「从本歌单移出」
    pub(super) collection: Option<CollectionId>,
    pub(super) library: Entity<LibraryEntity>,
    pub(super) list: WeakEntity<TrackList>,
}

pub(super) fn batch_bar(bar: BatchBar) -> impl IntoElement {
    let BatchBar {
        chosen_count,
        selected,
        rows,
        collection,
        library,
        list,
    } = bar;

    let mut row = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(theme::GAP_SM))
        .mx(px(theme::GAP_MD))
        .mb(px(theme::GAP_SM))
        .px(px(theme::GAP_MD))
        .py(px(theme::GAP_XS))
        .rounded(px(theme::RADIUS_CARD))
        .bg(theme::surface())
        .child(
            div()
                .text_size(px(theme::FONT_BODY))
                .text_color(if chosen_count == 0 {
                    theme::text_muted()
                } else {
                    theme::text()
                })
                .child(format!(
                    "已选 {} / {} 首",
                    thousands(chosen_count),
                    thousands(rows)
                )),
        )
        .child(div().flex_1());

    // 一首都没勾时只给「全选」：其余动作按选中曲目生效，点了也是空操作
    row = row.child(batch_button(
        "batch-all",
        "全选",
        list.clone(),
        TrackList::choose_all_visible,
    ));

    if chosen_count > 0 {
        row = row
            .child(batch_button(
                "batch-none",
                "清空选择",
                list.clone(),
                TrackList::clear_chosen,
            ))
            .child(batch_button(
                "batch-like",
                "收藏",
                list.clone(),
                TrackList::like_chosen,
            ));

        // 菜单项在闭包里现造：`PopupMenuItem` 内含 `Box<dyn Fn>`，不是 `Clone`。
        // ✓ 表示这一组曲目都在这个歌单里，再点一次是移出；批量场景没有「混合」态
        let in_list = list.clone();
        row = row.child(crate::common::menu::dropdown(
            crate::common::button::button_shell("batch-collection", ButtonKind::Outline)
                .child("加入歌单")
                .child(icon_inherit(Glyph::ChevronDown, 12.)),
            Anchor::TopRight,
            move |menu, _window, cx| {
                let lists: Vec<(CollectionId, String, bool)> = {
                    let read = library.read(cx);
                    // 菜单真正打开时才生成有序 ID：渲染路径不该为了一次点击扫全表。
                    let chosen = ordered_chosen(read, &selected);
                    read.collections()
                        .iter()
                        .map(|item| (item.id, item.name.clone(), all_inside(item, &chosen)))
                        .collect()
                };
                let mut menu = menu;
                for (id, name, inside) in lists {
                    let target = in_list.clone();
                    let mark = if inside { "✓ " } else { "" };
                    menu = menu.item(
                        crate::common::menu::long_item(format!("{mark}加入「{name}」")).on_click(
                            move |_, _, cx| {
                                target
                                    .update(cx, |list, cx| {
                                        if inside {
                                            list.remove_chosen_from(id, cx);
                                        } else {
                                            list.add_chosen_to(id, cx);
                                        }
                                    })
                                    .ok();
                            },
                        ),
                    );
                }
                let fresh = in_list.clone();
                menu.item(
                    PopupMenuItem::new("新建歌单并加入").on_click(move |_, _, cx| {
                        fresh.update(cx, TrackList::new_collection_with_chosen).ok();
                    }),
                )
            },
        ));

        // 「移出」只在正看着某个歌单时成立：其它作用域里没有「本歌单」可移
        if let Some(id) = collection {
            row = row.child(text_button(
                "batch-leave",
                "从本歌单移出",
                ButtonKind::Ghost,
                move |_, _, cx| {
                    list.update(cx, |list, cx| list.remove_chosen_from(id, cx))
                        .ok();
                },
            ));
        }
    }

    row
}

fn batch_button(
    id: &'static str,
    label: &'static str,
    list: WeakEntity<TrackList>,
    run: fn(&mut TrackList, &mut Context<TrackList>),
) -> impl IntoElement {
    text_button(id, label, ButtonKind::Ghost, move |_, _, cx| {
        list.update(cx, run).ok();
    })
}

/// 这一组曲目是不是全都在这个歌单里 —— 菜单项的 ✓ 与「再点一次是移出」都由它决定。
/// 空集合一律 `false`：一首都没勾时不该有任何一项显示成「已加入」。
fn all_inside(collection: &Collection, chosen: &[TrackId]) -> bool {
    !chosen.is_empty() && chosen.iter().all(|id| collection.contains(*id))
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`platform::*` 里的 gpui `test` 过程宏会遮蔽内置 `#[test]`。
    use lyre_core::{Collection, CollectionId, TrackId};

    use super::all_inside;

    fn collection_with(ids: &[u64]) -> Collection {
        let mut collection = Collection::new(CollectionId(1), "测试歌单");
        collection.add_all(ids.iter().map(|id| TrackId(*id)));
        collection
    }

    /// 全在里面才算 ✓，少一首就不算 —— 判错会把没加进去的歌从歌单里移出。
    #[test]
    fn a_checkmark_needs_every_chosen_track_inside() {
        let collection = collection_with(&[1, 2, 3]);
        assert!(all_inside(&collection, &[TrackId(1), TrackId(3)]));
        assert!(!all_inside(&collection, &[TrackId(1), TrackId(9)]));
    }

    #[test]
    fn an_empty_selection_never_shows_a_checkmark() {
        let collection = collection_with(&[1, 2, 3]);
        assert!(!all_inside(&collection, &[]));
    }
}
