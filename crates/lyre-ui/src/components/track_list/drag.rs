//! 拖拽排序：行首手柄、行内落点区，以及跟随鼠标的小卡片。

use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon, icon_inherit};
use crate::platform::*;
use crate::theme;

use super::HANDLE_W;

/// 拖拽载荷：被拖起来的是第几行（可见行号），与落点边界同一坐标。
struct DragRow {
    row: usize,
}

/// 行内落点的两个半边，各自对应一个插入边界。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DropEdge {
    /// 插在这一行之前
    Above,
    /// 插在这一行之后
    Below,
}

/// 落点半边 → 插入边界（可见行号，`rows` 就是末尾）。
/// 渲染用它摆线、落点用它报数、[`LibraryEntity::reorder_collection`] 用它改顺序，三处必须同一坐标。
fn boundary_of(index: usize, edge: DropEdge) -> usize {
    match edge {
        DropEdge::Above => index,
        DropEdge::Below => index + 1,
    }
}

/// 一行里的半个落点区。`boundary` 用可见行号表示「插到第几行之前」，可以是 `rows`（末尾）。
pub(super) fn drop_zone(
    index: usize,
    edge: DropEdge,
    row_height: f32,
    library: Entity<LibraryEntity>,
) -> AnyElement {
    let boundary = boundary_of(index, edge);
    div()
        .id(match edge {
            DropEdge::Above => ("row-drop-above", index),
            DropEdge::Below => ("row-drop-below", index),
        })
        .absolute()
        .left(px(0.))
        .right(px(0.))
        .h(px(row_height / 2.0))
        .when(edge == DropEdge::Above, |zone| zone.top(px(0.)))
        .when(edge == DropEdge::Below, |zone| zone.bottom(px(0.)))
        .drag_over::<DragRow>(move |style, _drag, _window, _cx| {
            let style = style.border_color(theme::control());
            match edge {
                DropEdge::Above => style.border_t_2(),
                DropEdge::Below => style.border_b_2(),
            }
        })
        .on_drop::<DragRow>(move |drag, _window, cx| {
            library.update(cx, |library, cx| {
                library.reorder_collection(drag.row, boundary, cx);
            });
        })
        .into_any_element()
}

/// 行首的拖拽手柄。gpui 要求拖拽影像是 `Entity<impl Render>`，所以 `on_drag` 里现造一个 [`DragGhost`]。
pub(super) fn handle_cell(index: usize, title: String, group: SharedString) -> AnyElement {
    div()
        .id(("row-handle", index))
        .flex_none()
        .w(px(HANDLE_W))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        // `CursorStyle::OpenHand` 就是 CSS 的 grab
        .cursor(CursorStyle::OpenHand)
        .text_color(theme::text_faint())
        .group_hover(group, |style| style.text_color(theme::text_muted()))
        .tooltip(crate::common::tooltip::builder("拖动排序"))
        .on_drag(DragRow { row: index }, move |_, _, _, cx| {
            let title = SharedString::from(title.clone());
            cx.new(|_cx| DragGhost { title })
        })
        .child(icon_inherit(Glyph::Grip, 14.))
        .into_any_element()
}

/// 跟着鼠标走的那张小卡片。gpui 的拖拽影像必须是一个视图实体，所以它只是个会画标题的壳。
struct DragGhost {
    title: SharedString,
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(theme::GAP_SM))
            .px(px(theme::GAP_MD))
            .py(px(theme::GAP_XS))
            .rounded(px(theme::RADIUS_MD))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .text_size(px(theme::FONT_TRACK))
            .text_color(theme::text())
            .child(icon(Glyph::Grip, 12., theme::text_muted()))
            .child(self.title.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{DropEdge, boundary_of};

    #[test]
    fn the_two_halves_of_a_row_are_adjacent_boundaries() {
        assert_eq!(boundary_of(0, DropEdge::Above), 0);
        assert_eq!(boundary_of(0, DropEdge::Below), 1);
        assert_eq!(boundary_of(3, DropEdge::Below), 4);
        // 相邻两行的接缝报的是同一个边界，线才不会跳
        assert_eq!(
            boundary_of(2, DropEdge::Below),
            boundary_of(3, DropEdge::Above)
        );
    }
}
