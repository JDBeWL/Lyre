//! 给滚动容器叠一条滚动条。gpui 自己从不画滚动条，列表只维护偏移量。

use crate::platform::*;

/// 叠在滚动容器之上的一条竖向滚动条。必须先用 `track_scroll` 把 `handle` 挂到容器上，
/// 否则读到的偏移量永远是零。泛型：虚拟列表用 `UniformListScrollHandle`，普通滚动区用 `ScrollHandle`。
pub fn scrollbar<H: ScrollbarHandle + Clone>(handle: &H, id: &'static str) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .child(
            Scrollbar::vertical(handle)
                .id(id)
                // 这条是滚动区的兄弟节点，不声明按自己的布局取视口就会照 handle 的边界算；
                // 外层的 `inset_0` 与它少一个，得到的都是不报错的错法。
                .viewport_from_layout()
                // 默认的 `Scrolling` 模式停手就淡出，淡出期间点不中。
                .mode(ScrollbarMode::Always),
        )
        .into_any_element()
}
