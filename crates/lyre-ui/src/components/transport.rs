//! 播放控制栏：上一曲 / 播放暂停 / 下一曲。
//! 只 `subscribe` 语义事件、不 `observe`：进度每 100ms 变一次，observe 会让按钮每秒重绘十次。

use crate::entities::{PlaybackEntity, PlaybackEvent};
use crate::icon::{Glyph, icon};
use crate::platform::*;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportEvent {
    TogglePlay,
    Prev,
    Next,
}

pub struct Transport {
    playback: Entity<PlaybackEntity>,
    playing: bool,
    has_track: bool,
    _sub: Subscription,
}

impl EventEmitter<TransportEvent> for Transport {}

impl Transport {
    pub fn new(playback: Entity<PlaybackEntity>, cx: &mut Context<Self>) -> Self {
        let (playing, has_track) = {
            let playback = playback.read(cx);
            (playback.is_playing(), playback.current().is_some())
        };

        let subscription = cx.subscribe(
            &playback,
            |this, _, event: &PlaybackEvent, cx| match event {
                PlaybackEvent::StateChanged | PlaybackEvent::TrackChanged => {
                    let playback = this.playback.read(cx);
                    this.playing = playback.is_playing();
                    this.has_track = playback.current().is_some();
                    cx.notify();
                }
                // `Finished` 由上层推进队列、设备列表只有底栏的设备菜单关心，这里都不处理
                PlaybackEvent::Finished
                | PlaybackEvent::VolumeChanged
                | PlaybackEvent::DevicesChanged
                | PlaybackEvent::OutputModeChanged => {}
            },
        );

        Self {
            playback,
            playing,
            has_track,
            _sub: subscription,
        }
    }
}

impl Render for Transport {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.has_track;

        // 三个按钮共享的外观壳子。tooltip 同时充当悬停提示和无障碍名称。
        let round = |id: &'static str, size: f32| {
            div()
                .id(id)
                .flex_none()
                .size(px(size))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
        };

        let mut prev = round("transport-prev", 32.).child(icon(
            Glyph::SkipBack,
            16.,
            if enabled {
                theme::text_muted()
            } else {
                theme::text_faint()
            },
        ));
        // 实心底配深色图标：黑压白 21:1，白三角压绿底只有 2.59:1，达不到 AA。
        let mut play = round("transport-play", 40.)
            .bg(if enabled {
                theme::control()
            } else {
                theme::track()
            })
            .child(icon(
                if self.playing {
                    Glyph::Pause
                } else {
                    Glyph::Play
                },
                if self.playing { 15. } else { 17. },
                if enabled {
                    theme::text_on_control()
                } else {
                    // 禁用态图标必须白字：`text_faint` 压 `track()` 底上只有 3.10:1，达不到 AA。
                    theme::text()
                },
            ));
        let mut next = round("transport-next", 32.).child(icon(
            Glyph::SkipForward,
            16.,
            if enabled {
                theme::text_muted()
            } else {
                theme::text_faint()
            },
        ));

        if enabled {
            prev = prev
                .cursor_pointer()
                .hover(|style| style.bg(theme::elevated()).text_color(theme::text()))
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.emit(TransportEvent::Prev);
                }))
                .tooltip(crate::common::tooltip::builder("上一曲"));
            // 播放键的事件只看 `enabled`，不看 `playing`。
            play = play
                .cursor_pointer()
                .hover(|style| style.bg(theme::control_hover()))
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.emit(TransportEvent::TogglePlay);
                }))
                .tooltip(crate::common::tooltip::builder(if self.playing {
                    "暂停"
                } else {
                    "播放"
                }));
            next = next
                .cursor_pointer()
                .hover(|style| style.bg(theme::elevated()).text_color(theme::text()))
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.emit(TransportEvent::Next);
                }))
                .tooltip(crate::common::tooltip::builder("下一曲"));
        } else {
            // 禁用态也要给读屏用户一个说明：按钮存在，只是还不可用。
            play = play
                .cursor_default()
                .tooltip(crate::common::tooltip::builder("先导入音乐才能播放"));
        }

        div()
            .flex()
            .items_center()
            .gap_3()
            .child(prev)
            .child(play)
            .child(next)
    }
}
