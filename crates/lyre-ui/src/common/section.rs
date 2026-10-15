//! 区级标题（侧栏的「你的音乐库」、设置页的分区、右面板的卡片标签）。

use crate::platform::*;
use crate::theme;

pub fn section_heading(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .min_w_0()
        .truncate()
        .text_size(px(theme::FONT_BODY))
        .font_weight(theme::WEIGHT_TITLE)
        .text_color(theme::text())
        .child(text.into())
}
