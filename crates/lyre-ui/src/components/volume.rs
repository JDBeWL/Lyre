//! 音量控制。轨道几何由 `canvas` 捕获换算比例，拖动中实时下发 —— 调音量不重建解码链。

use crate::components::settings_panel::SettingsEntity;
use crate::entities::{PlaybackEntity, PlaybackEvent};
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::theme;

const BAR_W: f32 = 84.0;
const HIT_H: f32 = 18.0;
const TRACK_H: f32 = 4.0;
/// 悬停 / 拖动时的轨道高度。
const TRACK_H_ACTIVE: f32 = 6.0;
/// 滑块的半径。轨道两端各让出这么多，见 [`ratio_in_track`]。
const THUMB_R: f32 = 5.0;
/// 拖动时浮出的数值气泡的宽度。定宽是为了它在 9% 和 100% 之间不左右跳。
const BUBBLE_W: f32 = 44.0;

/// 把连续比例吸附到整百分位，滑块位置与气泡数字一致。
fn snap(ratio: f32) -> f32 {
    (ratio * 100.0).round() / 100.0
}

/// 指针的横坐标 → 音量比例。轨道两端各让出一个滑块半径，滑块中心才不会探出轨道端点；
/// 越出这段行程的像素由 `clamp` 收在 0 / 1 上。
fn ratio_in_track(x: f32, origin: f32, width: f32) -> f32 {
    let usable = width - 2.0 * THUMB_R;
    if usable <= 0.0 {
        return 0.0;
    }
    snap(((x - origin - THUMB_R) / usable).clamp(0.0, 1.0))
}

pub struct VolumeControl {
    playback: Entity<PlaybackEntity>,
    settings: Entity<SettingsEntity>,
    /// 拖动中的预览值；`Some` 表示用户正按着。
    drag: Option<f32>,
    hovered: bool,
    track_bounds: Option<Bounds<Pixels>>,
    /// 静音前的音量，`Some` 表示当前处于静音。
    muted_from: Option<f32>,
    _sub: Subscription,
}

impl VolumeControl {
    pub fn new(
        playback: Entity<PlaybackEntity>,
        settings: Entity<SettingsEntity>,
        cx: &mut Context<Self>,
    ) -> Self {
        // subscribe 而不是 observe：音量不跟随进度，observe 会让它每 100ms 白重绘一次
        let subscription = cx.subscribe(&playback, |_, _, event: &PlaybackEvent, cx| {
            if matches!(event, PlaybackEvent::VolumeChanged) {
                cx.notify();
            }
        });
        let stored = settings.read(cx).settings();
        Self {
            playback,
            settings,
            drag: None,
            hovered: false,
            track_bounds: None,
            muted_from: stored.muted.then_some(stored.volume),
            _sub: subscription,
        }
    }

    fn ratio_at(&self, x: Pixels) -> f32 {
        let Some(bounds) = self.track_bounds else {
            return 0.0;
        };
        ratio_in_track(
            f32::from(x),
            f32::from(bounds.origin.x),
            f32::from(bounds.size.width),
        )
    }

    fn shown(&self, cx: &Context<Self>) -> f32 {
        self.drag.unwrap_or_else(|| self.playback.read(cx).volume())
    }

    /// 结束一次拖动，条内条外松手都走这里。拖动中已实时下发，这里收尾并落盘。
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.persist(cx);
            cx.notify();
        }
    }

    /// 落盘的是「用户要的音量」而不是此刻的增益：静音时 `muted_from` 记着前者。
    /// 只在一次操作结束时调用 —— 跟着拖动每帧写会把 `settings.json` 刷爆。
    fn persist(&self, cx: &mut Context<Self>) {
        let muted = self.muted_from.is_some();
        let volume = self
            .muted_from
            .unwrap_or_else(|| self.playback.read(cx).volume());
        self.settings
            .update(cx, |settings, cx| settings.set_volume(volume, muted, cx));
    }

    fn push(&self, ratio: f32, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |playback, cx| playback.set_volume(ratio, cx));
    }

    /// 按步长增减音量，键盘 `Ctrl + ↑/↓` 走这里。
    pub fn nudge(&mut self, delta: f32, cx: &mut Context<Self>) {
        let next = snap((self.playback.read(cx).volume() + delta).clamp(0.0, 1.0));
        if next > 0.01 {
            self.muted_from = None;
        }
        self.push(next, cx);
        self.persist(cx);
        cx.notify();
    }

    /// 静音 / 取消静音。`M` 键与点击喇叭都走这里。
    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(previous) = self.muted_from.take() {
            // 原音量本身就是 0（拖到底再静音）时给一个可听的默认值，否则取消静音没反应。
            let restore = if previous <= 0.01 { 0.5 } else { previous };
            self.push(restore, cx);
        } else {
            let current = self.playback.read(cx).volume();
            self.muted_from = Some(current);
            self.push(0.0, cx);
        }
        self.persist(cx);
        cx.notify();
    }
}

impl Render for VolumeControl {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let volume = self.shown(cx);
        let muted = self.muted_from.is_some() || volume <= 0.01;

        let glyph = match (muted, volume) {
            (true, _) => Glyph::VolumeMuted,
            (false, v) if v < 0.4 => Glyph::VolumeLow,
            _ => Glyph::VolumeHigh,
        };

        let speaker = div()
            .id("volume-mute")
            .flex_none()
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_PILL))
            .text_color(if muted {
                theme::text_faint()
            } else {
                theme::text_muted()
            })
            .cursor_pointer()
            .tab_index(crate::common::button::TAB_STOP)
            .focus_visible(theme::focus_ring)
            .role(Role::Button)
            .hover(|style| style.bg(theme::elevated()).text_color(theme::text()))
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.toggle_mute(cx);
            }))
            .child(icon_inherit(glyph, 16.))
            .aria_label(if muted { "取消静音" } else { "静音" })
            .tooltip(crate::common::tooltip::builder(if muted {
                "取消静音"
            } else {
                "静音"
            }));

        let weak = cx.entity().downgrade();
        let capture = move |bounds: Bounds<Pixels>, _window: &mut Window, cx: &mut App| {
            weak.update(cx, |this, _| this.track_bounds = Some(bounds))
                .ok();
        };

        let active = self.hovered || self.drag.is_some();
        let track_height = if active { TRACK_H_ACTIVE } else { TRACK_H };

        let bar = div()
            .id("volume-bar")
            // 只进 Tab 序列；实际操作走全局的 `Ctrl + ↑/↓`。
            .tab_index(crate::common::button::TAB_STOP)
            .focus_visible(theme::focus_ring)
            .role(Role::Slider)
            .aria_label("音量")
            .relative()
            .w(px(BAR_W))
            .h(px(HIT_H))
            .flex()
            .items_center()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    let ratio = this.ratio_at(event.position.x);
                    this.drag = Some(ratio);
                    // 拖离 0 就等于取消静音，否则状态自相矛盾。
                    if ratio > 0.01 {
                        this.muted_from = None;
                    }
                    this.push(ratio, cx);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.drag.is_some() {
                    let ratio = this.ratio_at(event.position.x);
                    this.drag = Some(ratio);
                    if ratio > 0.01 {
                        this.muted_from = None;
                    }
                    this.push(ratio, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.end_drag(cx);
                }),
            )
            // 条外松手也要结束拖动：`on_mouse_up` 要求松手时元素仍被命中，横着甩出去就收不到，
            // 那样 `drag` 一直留着，轨道和滑块卡在「正在拖」的样子上。
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.end_drag(cx);
                }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if this.hovered != *hovered {
                    this.hovered = *hovered;
                    cx.notify();
                }
            }))
            .child(
                // 轨道两端各让出一个滑块半径，行程与 `ratio_in_track` 的换算一致：
                // 指针在哪，滑块中心就到哪。
                div()
                    .absolute()
                    .left(px(THUMB_R))
                    .right(px(THUMB_R))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
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
                                    .w(relative(volume))
                                    .rounded_full()
                                    .bg(if muted {
                                        theme::text_faint()
                                    } else {
                                        theme::control()
                                    }),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(relative(0.5))
                                    .mt(px(-5.))
                                    .left(relative(volume))
                                    .ml(px(-5.))
                                    .size(px(10.))
                                    .rounded_full()
                                    .bg(theme::text())
                                    .opacity(if active { 1.0 } else { 0.0 }),
                            ),
                    ),
            )
            .child(
                canvas(
                    capture,
                    |_bounds: Bounds<Pixels>, _state: (), _window: &mut Window, _cx: &mut App| {},
                )
                .absolute()
                .size_full(),
            );

        let bubble = self.drag.map(|value| {
            // 跟着滑块中心走，但两端夹进轨道内：不然 0% / 100% 时气泡会探出底栏边缘。
            let center = THUMB_R + value * (BAR_W - 2.0 * THUMB_R);
            let left = (center - BUBBLE_W / 2.0).clamp(0.0, BAR_W - BUBBLE_W);
            div()
                .absolute()
                .bottom(px(HIT_H + 4.0))
                .left(px(left))
                .w(px(BUBBLE_W))
                .flex()
                .justify_center()
                .py(px(theme::GAP_XS))
                .rounded(px(theme::RADIUS_SM))
                .bg(theme::elevated())
                .border_1()
                .border_color(theme::border())
                .text_size(px(theme::FONT_MICRO))
                .text_color(theme::text())
                .child(format!("{}%", (value * 100.0).round() as i32))
        });

        // `Option::into_iter` 就是「有就加、没有就不加」，这个版本没有 `maybe_child`。
        let bar = bar.children(bubble.map(IntoElement::into_any_element));

        div()
            .flex()
            .items_center()
            .gap(px(theme::GAP_XS))
            .child(speaker)
            .child(bar)
    }
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`crate::platform::*` 带进来的 gpui `test` 宏会遮蔽内置 `#[test]`。
    use super::{BAR_W, THUMB_R, ratio_in_track, snap};

    #[test]
    fn the_two_ends_of_the_track_are_exactly_0_and_100_percent() {
        assert_eq!(ratio_in_track(BAR_W, 0.0, BAR_W), 1.0);
        assert_eq!(ratio_in_track(0.0, 0.0, BAR_W), 0.0);
        assert_eq!(ratio_in_track(BAR_W + 40.0, 0.0, BAR_W), 1.0);
        assert_eq!(ratio_in_track(-40.0, 0.0, BAR_W), 0.0);
    }

    #[test]
    fn the_pointer_and_the_thumb_center_coincide() {
        for percent in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let travel = THUMB_R + percent * (BAR_W - 2.0 * THUMB_R);
            let got = ratio_in_track(travel, 0.0, BAR_W);
            assert!(
                (got - percent).abs() < 0.005,
                "指针在 {travel} 应当得到 {percent}，实得 {got}"
            );
        }
    }

    #[test]
    fn values_snap_to_whole_percent() {
        assert_eq!(snap(0.99881), 1.0);
        assert_eq!(snap(0.504), 0.5);
        assert_eq!(snap(0.0), 0.0);
    }

    /// 轨道被压到比两个滑块半径还窄时不能除出负数或 NaN。
    #[test]
    fn a_degenerate_track_width_yields_silence_instead_of_nan() {
        assert_eq!(ratio_in_track(1.0, 0.0, 4.0), 0.0);
        assert!(ratio_in_track(1.0, 0.0, 0.0).is_finite());
    }
}
