//! 主区背景上的竖直渐变色带，铺在内容背后；色相由 `theme::hero_tint` 从字符串同步派生。

use crate::platform::*;
use crate::theme;

/// 顶部持平段高度：这个 fork 的 `linear_gradient` 只有两个停靠点，做不出「先持平再淡出」，只能在渐变上方压一条实色。
const HERO_PLATEAU_H: f32 = 64.0;

/// 收尾实色带的厚度，同时是渐变段的底边内缩量。两者必须相等：
/// 渐变要在收尾带上边缘正好淡到 alpha 0，否则那条线上残留的色差会被切成硬边。
const HERO_TAIL_H: f32 = theme::GAP_XL;

/// 把渐变色带铺在主区内容背后，作为主区容器的首个 `absolute` 子元素（`absolute` 不占垂直空间）。
pub fn hero_band(seed: Option<&str>) -> impl IntoElement {
    let tint = theme::hero_tint(seed);

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(HERO_PLATEAU_H))
                .bg(tint),
        )
        // 色带不参与命中测试靠的是「无 id、不绑事件」，gpui 没有 `pointer-events`，别给它加 `.on_click`。
        .child(
            div()
                .absolute()
                .top(px(HERO_PLATEAU_H))
                .left_0()
                .right_0()
                // 收到的不是 `bottom_0`：淡出必须在这一条的上边缘就归零，
                // 否则下面那条实色带会把还没淡完的渐变切断。
                .bottom(px(HERO_TAIL_H))
                .bg(linear_gradient(
                    // 这个 fork 的 Windows 渲染器把 angle 减 90 再算方向，且屏幕 y 轴向下，
                    // 所以 `percentage = 0` 的停靠点落在色带底部：停靠点书写顺序与视觉顺序相反。
                    0.0,
                    linear_color_stop(tint.opacity(0.0), 0.0),
                    linear_color_stop(tint, 1.0),
                )),
        )
        // 收尾的 `bg` 实色层：alpha 混合在深色底上淡出没干净，会留一层极淡的雾。
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(HERO_TAIL_H))
                .bg(theme::bg()),
        )
}
