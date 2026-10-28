//! 键盘光标：`↑↓` 只移动光标，`Enter` 才播。默认跟着播放走（[`LibraryEntity::follow_playback`]）。
//! 键盘路径的光标变动都走 [`LibraryEntity::set_focus`]，
//! 它是 [`super::LibraryEvent::FocusMoved`] 的唯一发出者。

use lyre_core::{Track, TrackId};

use crate::platform::*;

use super::{LibraryEntity, LibraryEvent};

impl LibraryEntity {
    pub fn focus(&self) -> Option<TrackId> {
        self.focus
    }

    /// 光标底色该不该画。
    pub fn shows_cursor(&self) -> bool {
        self.cursor_visible
    }

    /// 移动光标（键盘路径）。发 `FocusMoved` 让列表滚过去 —— 事件带的是可见行号而非曲库
    /// 下标，过滤状态下两者不同。换曲引起的光标移动走 `Self::follow_playback`：那条不发事件。
    pub fn set_focus(&mut self, id: TrackId, cx: &mut Context<Self>) {
        let row = self.row_of(id);
        self.set_focus_at(id, row, cx);
    }

    /// 行号已经知道时直接落座，省掉一次 `row_of` 反查。
    fn set_focus_at(&mut self, id: TrackId, row: Option<usize>, cx: &mut Context<Self>) {
        // 要放在早返回之前：首行再按 `↑` 落点没变，但用户在用键盘，底色该亮。
        self.cursor_visible = true;
        if self.focus == Some(id) {
            return;
        }
        self.focus = Some(id);
        if let Some(row) = row {
            cx.emit(LibraryEvent::FocusMoved(row));
        }
        cx.notify();
    }

    /// 换曲时把光标挪到这一首，不发 `FocusMoved`：自动换曲不该把列表从用户手上拽走。
    /// 那首不在可见列表里就清成 `None`，`↑↓` 与 `Enter` 都有兜底。
    pub(super) fn follow_playback(&mut self, id: TrackId, cx: &mut Context<Self>) {
        // 这次是播放自己在走，不是用户在按键盘 —— 底色不该跟着冒出来。
        self.cursor_visible = false;
        let target = if self.row_of(id).is_some() {
            Some(id)
        } else {
            None
        };
        if self.focus != target {
            self.focus = target;
            cx.notify();
        }
    }

    /// 光标没初始化时落到第一行，否则按 `delta` 移动并夹在范围内。
    /// 在可见行空间里移动（过滤生效时与曲库下标不同），且不自动播放。
    fn move_focus(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.visible_len();
        if rows == 0 {
            return;
        }

        let next_row = match self.focus_row() {
            // `delta` 是 isize 只为表达方向，行号全程用 usize 饱和运算，避免来回转换。
            Some(current) => {
                let step = delta.unsigned_abs();
                let moved = if delta >= 0 {
                    current.saturating_add(step)
                } else {
                    current.saturating_sub(step)
                };
                moved.min(rows - 1)
            }
            // 没有光标时：向下从第一行开始，向上从最后一行开始
            None => {
                if delta >= 0 {
                    0
                } else {
                    rows - 1
                }
            }
        };

        let target = self.track_at_row(next_row).map(|t| t.id);
        if let Some(id) = target {
            self.set_focus_at(id, Some(next_row), cx);
        }
    }

    /// 第 `row` 行的曲目（可见投影内）。
    pub fn track_at_row(&self, row: usize) -> Option<&Track> {
        let index = match &self.visible {
            Some(rows) => *rows.get(row)?,
            None => row,
        };
        self.playlist.tracks().get(index)
    }

    pub fn focus_next(&mut self, cx: &mut Context<Self>) {
        self.move_focus(1, cx);
    }

    pub fn focus_prev(&mut self, cx: &mut Context<Self>) {
        self.move_focus(-1, cx);
    }

    /// 跳到第一行可见项。过滤生效时不是曲库第一首。
    pub fn focus_first(&mut self, cx: &mut Context<Self>) {
        if let Some(track) = self.track_at_row(0) {
            let id = track.id;
            self.set_focus_at(id, Some(0), cx);
        }
    }

    pub fn focus_last(&mut self, cx: &mut Context<Self>) {
        let last = self.visible_len().saturating_sub(1);
        if let Some(track) = self.track_at_row(last) {
            let id = track.id;
            self.set_focus_at(id, Some(last), cx);
        }
    }

    /// 播放光标所在曲目。返回是否有事发生（供调用方决定要不要处理 `Step`）。
    /// 光标停在一首引擎播不了的曲目上时返回 `None`（并弹一条说明）。
    pub fn play_focused(&mut self, cx: &mut Context<Self>) -> Option<Track> {
        let id = self.focus?;
        self.activate(id, cx)
    }

    /// 从可见行里挑第一首引擎能播的并开始播放；一首都没有时只提示。
    /// 空格在什么都没装载时走这条，而不是走光标那一首：光标可能停在一首播不了的曲目上。
    pub fn play_first_playable(&mut self, cx: &mut Context<Self>) -> Option<Track> {
        let found = (0..self.visible_len()).find_map(|row| {
            self.track_at_row(row)
                .filter(|track| track.is_playable())
                .map(|track| track.id)
        });

        if let Some(id) = found {
            return self.activate(id, cx);
        }

        cx.emit(LibraryEvent::Failed(
            "曲库里没有引擎能播放的曲目".to_owned(),
        ));
        None
    }
}
