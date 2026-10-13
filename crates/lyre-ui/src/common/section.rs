//! 区级标题（侧栏的「你的音乐库」、设置页的分区、右面板的卡片标签）。
//! 页级标题不在这里 —— 那是压在渐变色带上的 `FONT_HERO` 那一行，各页面自己画。

use crate::platform::*;
use crate::theme;

/// 14px 粗白，取 `FONT_BODY` + `WEIGHT_TITLE`。
pub fn section_heading(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .min_w_0()
        .truncate()
        .text_size(px(theme::FONT_BODY))
        .font_weight(theme::WEIGHT_TITLE)
        .text_color(theme::text())
        .child(text.into())
}
