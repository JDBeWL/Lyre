//! 进度条 —— 全项目唯一按 10Hz 重绘的组件，单独成 `Entity` 自己 observe 播放实体，把重绘隔离在这条上。
//! 拖动期间只显示本地预览值，松手才下发一次 `Seek`：seek 会重建解码链，拖动过程中连续下发会卡顿并爆音。

use std::time::Duration;

use lyre_core::format_duration;

use crate::entities::PlaybackEntity;
use crate::platform::*;
use crate::theme;

const TRACK_H: f32 = 4.0;
const TRACK_H_ACTIVE: f32 = 8.0;
/// 命中区高度：不能高到盖住底栏里上下的其它元素。
const HIT_H: f32 = 22.0;

pub struct SeekBar {
    playback: Entity<PlaybackEntity>,
    /// 拖动中的预览比例；`Some` 表示用户正按着。
    drag: Option<f32>,
    /// 鼠标悬停的位置比例。与 `drag` 分开，因为两者可以不同时成立。
    hover: Option<f32>,
    /// 轨道在窗口中的绝对几何，由 `canvas` 的 prepaint 捕获。
    track_bounds: Option<Bounds<Pixels>>,
    _sub: Subscription,
}

impl SeekBar {
    pub fn new(playback: Entity<PlaybackEntity>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&playback, |_, _, cx| cx.notify());
        Self {
            playback,
            drag: None,
            hover: None,
            track_bounds: None,
            _sub: subscription,
        }
    }

    fn ratio_at(&self, x: Pixels) -> f32 {
        let Some(bounds) = self.track_bounds else {
            return 0.0;
        };
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return 0.0;
        }
        (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0)
    }

    /// 结束拖动并把预览位置真正下发。条内松手和条外松手都走这里，两条路径必须等价。
    /// `drag` 为 `None` 时什么都不做：`on_mouse_up_out` 会被窗口里任何一次无关点击触发。
    fn commit_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(preview) = self.drag.take() {
            let duration = self.playback.read(cx).duration();
            self.playback
                .update(cx, |playback, _| playback.seek(duration.mul_f32(preview)));
            cx.notify();
        }
    }
}

impl Render for SeekBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (playing_ratio, position, duration) = {
            let playback = self.playback.read(cx);
            (
                playback.progress(),
                playback.position(),
                playback.duration(),
            )
        };

        let ratio = self.drag.or(self.hover).unwrap_or(playing_ratio);
        let shown = match self.drag {
            Some(preview) => duration.mul_f32(preview),
            None => match self.hover {
                Some(preview) => duration.mul_f32(preview),
                None => position,
            },
        };

        let active = self.drag.is_some() || self.hover.is_some();
        let track_height = if active { TRACK_H_ACTIVE } else { TRACK_H };

        let weak = cx.entity().downgrade();
        let capture = move |bounds: Bounds<Pixels>, _window: &mut Window, cx: &mut App| {
            weak.update(cx, |this, _| this.track_bounds = Some(bounds))
                .ok();
        };

        div()
            .flex()
            .items_center()
            .gap(px(theme::GAP_MD))
            .w_full()
            .child(time_label(
                shown,
                if active {
                    theme::text()
                } else {
                    theme::text_muted()
                },
                true,
            ))
            .child(
                div()
                    .id("seek-bar")
                    // 只进 Tab 序列；聚焦后的操作走全局 ←/→（步长在 `shortcuts::SEEK_SECONDS`），这里不另接一套。
                    .tab_index(crate::common::button::TAB_STOP)
                    .focus_visible(theme::focus_ring)
                    .role(Role::Slider)
                    .aria_label("播放进度")
                    .relative()
                    .flex_1()
                    .h(px(HIT_H))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            // 按下即从整条轨道取值，不要求先精确抓到滑块。
                            this.drag = Some(this.ratio_at(event.position.x));
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        let ratio = this.ratio_at(event.position.x);
                        this.hover = Some(ratio);
                        if this.drag.is_some() {
                            this.drag = Some(ratio);
                        }
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            this.commit_drag(cx);
                        }),
                    )
                    // 条外松手也要结束拖动：`on_mouse_up` 要求松手时元素仍被命中，
                    // 甩出这条 22px 命中区再松手就收不到，`drag` 会一直留着、进度显示冻结在预览值。
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            this.commit_drag(cx);
                        }),
                    )
                    // 移出要清掉 hover，否则预览会一直挂着不走。
                    .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                        if !*hovered && this.hover.is_some() {
                            this.hover = None;
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(track_height))
                            .rounded_full()
                            .bg(theme::track())
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .h_full()
                                    .w(relative(ratio))
                                    .rounded_full()
                                    .bg(theme::control()),
                            )
                            .child(
                                // 滑块用 50% + 负 margin 居中，轨道增厚时自动跟着走。
                                div()
                                    .absolute()
                                    .top(relative(0.5))
                                    .mt(px(-6.))
                                    .left(relative(ratio))
                                    .ml(px(-6.))
                                    .size(px(12.))
                                    .rounded_full()
                                    .bg(theme::text())
                                    .opacity(if active { 1.0 } else { 0.0 }),
                            ),
                    )
                    .child(
                        canvas(
                            capture,
                            |_bounds: Bounds<Pixels>,
                             _state: (),
                             _window: &mut Window,
                             _cx: &mut App| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .child(time_label(duration, theme::text_muted(), false))
    }
}

fn time_label(value: Duration, color: Rgba, align_right: bool) -> impl IntoElement {
    // 固定宽度：时间字符串长度会变（`9:59` ↔ `1:00:00`），不固定会每秒把进度条左右推一下。
    let label = div()
        .flex_none()
        .w(px(52.))
        .flex()
        .text_size(px(theme::FONT_CAPTION))
        .text_color(color)
        .child(format_duration(value));

    if align_right {
        label.justify_end()
    } else {
        label.justify_start()
    }
}
