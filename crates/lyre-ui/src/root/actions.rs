//! 根视图上的动作：一次用户意图落到多个实体上，这里不持有状态。
//! 破坏性操作的撤销入口收敛在这里。

use std::time::Duration;

use crate::entities::{LibraryEntity, PlaybackEntity, PlaybackState, Step};
use crate::platform::*;

use super::RootView;

/// 键盘焦点是否落在可 Tab 到的控件上 —— gpui 内建会把 Enter / Space 转成聚焦控件自己的 `on_click`，这两个键又绑着播放动作。
/// 还要「上一次输入是键盘」：鼠标点过按钮同样留下焦点，那时不该抢走 Space。
pub(super) fn control_has_focus(window: &Window, cx: &App) -> bool {
    window.last_input_was_keyboard() && window.focused(cx).is_some_and(|handle| handle.tab_stop)
}

impl RootView {
    /// 什么都没装载时音频线程会忽略 `PlayerCommand::TogglePlay`，所以这里先送一首进播放器。
    /// 队列走到头后也要补回 `now_playing`，否则列表一行都不亮。
    pub(super) fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let (current, stopped) = {
            let playback = self.playback.read(cx);
            (
                playback.current().map(|track| track.id),
                playback.state() == PlaybackState::Stopped,
            )
        };

        if let Some(id) = current {
            if stopped {
                self.library
                    .update(cx, |library, cx| library.set_now_playing(Some(id), cx));
            }
            self.playback.update(cx, PlaybackEntity::toggle_play);
            return;
        }

        // 没有键盘光标时先落到第一首，否则 play_focused 无事可做
        if self.library.read(cx).focus().is_none() {
            self.library.update(cx, LibraryEntity::focus_first);
        }
        self.library.update(cx, LibraryEntity::play_focused);
    }

    /// 队列推进时同步 `now_playing`，否则高亮停在上一首。
    pub(super) fn apply_step(&mut self, step: Step, cx: &mut Context<Self>) {
        match step {
            Step::Play(track) => {
                let id = track.id;
                self.library
                    .update(cx, |library, cx| library.set_now_playing(Some(id), cx));
                self.playback
                    .update(cx, |playback, cx| playback.load(track, true, cx));
            }
            Step::Restart => {
                self.playback.update(cx, |playback, _| playback.restart());
            }
            Step::Stop => {
                self.library
                    .update(cx, |library, cx| library.set_now_playing(None, cx));
                self.playback.update(cx, PlaybackEntity::stop);
            }
        }
    }

    pub(super) fn nudge_seek(&mut self, delta: i64, cx: &mut Context<Self>) {
        self.playback.update(cx, |playback, _| {
            let current = playback.position().as_millis() as i64;
            let total = playback.duration().as_millis() as i64;
            let target = (current + delta).clamp(0, total.max(0)) as u64;
            playback.seek(Duration::from_millis(target));
        });
    }

    pub(super) fn remove_focused(&mut self, cx: &mut Context<Self>) {
        let removed = self.library.update(cx, LibraryEntity::remove_focused);
        if let Some((track, index)) = removed {
            self.offer_remove_undo(track, index, cx);
        }
    }

    /// 弹「已移除 + 撤销」，撤销时把曲目插回原处。
    /// 闭包要 `'static`，所以把 `Entity` 句柄克隆进去。
    pub(super) fn offer_remove_undo(
        &mut self,
        track: lyre_core::Track,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let title = track.title.clone();
        let library = self.library.clone();

        self.toast.update(cx, |toast, cx| {
            toast.show_undo(
                format!("已从曲库移除《{title}》"),
                move |_window, cx| {
                    // 撤销只动曲库，正在播的音频不受影响
                    library.update(cx, |library, cx| library.restore(&track, index, cx));
                },
                cx,
            );
        });
    }

    pub(super) fn offer_clear_undo(
        &mut self,
        tracks: Vec<lyre_core::Track>,
        cx: &mut Context<Self>,
    ) {
        let count = tracks.len();
        let library = self.library.clone();

        self.toast.update(cx, |toast, cx| {
            toast.show_undo(
                format!("已清空曲库（{count} 首）"),
                move |_window, cx| {
                    library.update(cx, |library, cx| library.restore_all(tracks, cx));
                },
                cx,
            );
        });
    }
}
