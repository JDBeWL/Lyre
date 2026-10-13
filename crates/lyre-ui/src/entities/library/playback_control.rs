//! 随机与循环是两条轴，四态 `PlayMode` 装不下它们的组合，所以这里存 [`RepeatState`] 与 `shuffle`。
//! 由 [`LibraryEntity::effective_mode`] 折算成 `PlayMode`、[`LibraryEntity::set_mode`] 拆回来，两个方向必须成对改。

use lyre_core::{Advance, PlayMode, Track, TrackId};

use crate::platform::*;

use super::{LibraryEntity, LibraryEvent};

/// 队列推进的结论，由 `RootView` 翻译成对 `PlaybackEntity` 的实际操作。
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Play(Track),
    /// 回到当前曲目开头
    Restart,
    /// 队列到头，停下
    Stop,
}

/// 循环档位。与随机轴各管一头，组合由 `LibraryEntity::effective_mode` 折算成 [`PlayMode`]。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RepeatState {
    /// 不循环：走到队尾停下
    #[default]
    Off,
    /// 列表循环：走到队尾绕回开头
    All,
    /// 单曲循环：一首放完原地重播
    One,
}

impl RepeatState {
    /// 循环按钮的下一档：关 → 列表 → 单曲 → 关
    pub fn next_in_cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "不循环",
            Self::All => "列表循环",
            Self::One => "单曲循环",
        }
    }
}

impl LibraryEntity {
    /// 列表用来标注「播放中」的那一首
    pub fn now_playing(&self) -> Option<TrackId> {
        self.now_playing
    }

    /// 上层「把某首送进播放器」时调用，只在真的变了时 `notify`，顺带把光标也挪过来。
    pub fn set_now_playing(&mut self, id: Option<TrackId>, cx: &mut Context<Self>) {
        if self.now_playing != id {
            self.now_playing = id;
            cx.notify();
        }
        if let Some(id) = id {
            self.follow_playback(id, cx);
        }
    }

    pub fn current_id(&self) -> Option<TrackId> {
        self.playlist.current_id()
    }

    /// 当前播放队列的长度，队列面板的「共 N 首」用它而不是 [`Self::len`]。
    pub fn queue_len(&self) -> usize {
        self.playlist.order().len()
    }

    /// 从一张专辑卡开始播放：队列定为这张专辑的曲目，再跳到这一首；查不到这张专辑时不起队列。
    pub fn activate_album(
        &mut self,
        title: &str,
        id: TrackId,
        cx: &mut Context<Self>,
    ) -> Option<Track> {
        let queue = self
            .albums()
            .iter()
            .find(|group| group.title == title)
            .map(|group| group.tracks.clone());
        self.playlist.set_queue(queue, self.seed);
        self.activate(id, cx)
    }

    /// 从一位艺术家卡开始播放：队列定为这位艺术家的曲目，查不到时不起队列。
    pub fn activate_artist(
        &mut self,
        name: &str,
        id: TrackId,
        cx: &mut Context<Self>,
    ) -> Option<Track> {
        let queue = self
            .artists()
            .iter()
            .find(|group| group.name == name)
            .map(|group| group.tracks.clone());
        self.playlist.set_queue(queue, self.seed);
        self.activate(id, cx)
    }

    /// 从当前可见列表开始播放：先把队列定为可见行（按显示顺序），再跳到这一首。
    /// 可见列表恰好就是整库时不起队列，保持「队列随曲库走」的语义。
    pub fn activate_in_view(&mut self, id: TrackId, cx: &mut Context<Self>) -> Option<Track> {
        let whole_library = !self.scope.is_scoped() && !self.has_query();
        let queue = if whole_library {
            None
        } else {
            let rows = self.visible_ids(0..self.visible_len());
            // 空列表不该把队列清空（调用方多半是在列表里点的，正常走不到）
            if rows.is_empty() { None } else { Some(rows) }
        };
        self.playlist.set_queue(queue, self.seed);
        self.activate(id, cx)
    }

    /// 选中某首并广播「该播它了」，返回被选中的曲目。只跳转，不改队列内容。
    pub fn activate(&mut self, id: TrackId, cx: &mut Context<Self>) -> Option<Track> {
        if !self.playlist.jump_to(id) {
            return None;
        }
        self.set_focus(id, cx);
        // 鼠标点不是键盘导航：光标跟过来了，底色不该因此亮起来（`↑↓` 走 `move_focus` 才会亮）。
        self.cursor_visible = false;
        let track = self.playlist.current()?.clone();
        self.now_playing = Some(id);
        cx.emit(LibraryEvent::Play(track.clone()));
        cx.notify();
        Some(track)
    }

    pub fn step_next(&mut self) -> Step {
        match self.playlist.next() {
            Advance::To(_) => self.resolve_current(),
            Advance::Restart => Step::Restart,
            Advance::Stop => Step::Stop,
        }
    }

    pub fn step_prev(&mut self) -> Step {
        match self.playlist.prev() {
            Advance::To(_) => self.resolve_current(),
            Advance::Restart => Step::Restart,
            Advance::Stop => Step::Stop,
        }
    }

    /// 一首歌自然播完该去哪。单曲循环在这里生效。
    pub fn step_on_end(&mut self) -> Step {
        match self.playlist.on_track_ended() {
            Advance::To(_) => self.resolve_current(),
            Advance::Restart => Step::Restart,
            Advance::Stop => Step::Stop,
        }
    }

    pub fn is_shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat_state(&self) -> RepeatState {
        self.repeat
    }

    /// 两条轴折算成四态：随机 + 列表循环取随机序（循环靠 [`Playlist::set_wrap_at_end`] 另传），随机 + 单曲循环取单曲循环。
    fn effective_mode(&self) -> PlayMode {
        match (self.shuffle, self.repeat) {
            (_, RepeatState::One) => PlayMode::RepeatOne,
            (false, RepeatState::Off) => PlayMode::Sequential,
            (false, RepeatState::All) => PlayMode::RepeatAll,
            (true, RepeatState::Off | RepeatState::All) => PlayMode::Shuffle,
        }
    }

    /// 写入折算后的模式；`reshuffle` 为真时推进洗牌种子（= 刚进入随机，换一副牌）。
    fn apply_mode(&mut self, reshuffle: bool, cx: &mut Context<Self>) {
        if reshuffle {
            self.seed = self
                .seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
        }
        self.playlist.set_mode(self.effective_mode(), self.seed);
        self.playlist
            .set_wrap_at_end(matches!(self.repeat, RepeatState::All));
        cx.notify();
    }

    /// 随机按钮：设定随机轴的开 / 关
    pub fn set_shuffle(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        self.apply_mode(on, cx);
    }

    /// `R` 键：非随机直接进随机、已是随机则回到顺序，不经过列表 / 单曲循环
    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        let on = !self.shuffle;
        self.set_shuffle(on, cx);
    }

    /// 循环按钮：关 → 列表 → 单曲 → 关，不影响随机轴
    pub fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        self.repeat = self.repeat.next_in_cycle();
        self.apply_mode(false, cx);
    }

    /// `S` 键的四态整体循环，同时把结果写回两条轴
    pub fn cycle_mode(&mut self, cx: &mut Context<Self>) {
        let next = self.playlist.mode().next_in_cycle();
        self.set_axes_from_mode(next);
        self.apply_mode(true, cx);
    }

    /// 直接设四态模式，拆成两条轴
    pub fn set_mode(&mut self, mode: PlayMode, cx: &mut Context<Self>) {
        self.set_axes_from_mode(mode);
        self.apply_mode(true, cx);
    }

    /// 由四态模式反推两条轴，不是 [`Self::effective_mode`] 的逆：单曲循环落在 `(false, One)`，那一档排不出随机序。
    fn set_axes_from_mode(&mut self, mode: PlayMode) {
        (self.shuffle, self.repeat) = match mode {
            PlayMode::Sequential => (false, RepeatState::Off),
            PlayMode::Shuffle => (true, RepeatState::Off),
            PlayMode::RepeatAll => (false, RepeatState::All),
            PlayMode::RepeatOne => (false, RepeatState::One),
        };
    }

    fn resolve_current(&self) -> Step {
        match self.playlist.current() {
            Some(track) => Step::Play(track.clone()),
            None => Step::Stop,
        }
    }
}
