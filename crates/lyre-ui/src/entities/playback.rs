//! 播放实体：向音频线程下发 [`PlayerCommand`]，把 [`PlayerEvent`] 翻成 [`PlaybackEvent`]。

use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use lyre_audio::{AudioHandle, PlaybackState, PlayerCommand, PlayerEvent, Snapshot};
use lyre_core::Track;

use crate::platform::*;

/// 事件泵节拍。音频线程每 50ms 塞一条进度，这里每拍排空积压、只 `notify()` 一次，把重绘钉在 10Hz。
const PUMP_INTERVAL: Duration = Duration::from_millis(100);

/// 对外广播的语义事件（进度不在此列，它走 [`PlaybackEntity::position`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackEvent {
    /// 换曲了（含首次装载）
    TrackChanged,
    /// 播放 / 暂停状态变了
    StateChanged,
    /// 音量变了
    VolumeChanged,
    /// 一首歌自然播完了 —— 由上层决定下一曲还是循环
    Finished,
}

pub struct PlaybackEntity {
    /// 音频线程起不来时为 `None`：界面照常可用，只是没有声音。
    handle: Option<AudioHandle>,
    snapshot: Arc<Snapshot>,

    state: PlaybackState,
    current: Option<Track>,
    duration: Duration,
    volume: f32,
    error: Option<String>,

    /// 事件泵。持有它任务才不会被提前中断。
    _pump: Task<()>,
}

impl EventEmitter<PlaybackEvent> for PlaybackEntity {}

impl PlaybackEntity {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = mpsc::channel();
        // 线程起不来（资源耗尽）不是致命错误：退化成没声音但界面可用，原因留在 `error` 里显示。
        let mut startup_error = None;
        let handle = match AudioHandle::spawn(tx) {
            Ok(handle) => Some(handle),
            Err(err) => {
                let message = err.to_string();
                log::error!("音频线程创建失败，本次运行将没有声音：{message}");
                startup_error = Some(message);
                None
            }
        };
        // 没有句柄就没有共享快照，自己造一个默认的：进度恒为 0，界面不至于崩。
        let snapshot = handle
            .as_ref()
            .map_or_else(|| Arc::new(Snapshot::default()), AudioHandle::snapshot);
        let volume = snapshot.volume();

        // `background_executor()` 返回借用，必须 clone 出所有权值才能带进 'static 任务
        let executor = cx.background_executor().clone();
        let pump = cx.spawn(async move |this, cx| {
            let rx = rx;
            // 进度只写进共享快照、不发事件，所以「位置变了」得自己察觉，否则没人重绘
            let mut shown = Duration::ZERO;
            loop {
                executor.timer(PUMP_INTERVAL).await;
                let mut batch: Vec<PlayerEvent> = Vec::new();
                let mut disconnected = false;
                loop {
                    match rx.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
                let got_events = !batch.is_empty();

                let alive = this
                    .update(cx, |this, cx| {
                        let mut emitted: Vec<PlaybackEvent> = Vec::new();
                        for event in batch {
                            this.apply(event, &mut emitted);
                        }
                        let moved = this.snapshot.position() != shown;
                        if moved {
                            shown = this.snapshot.position();
                        }
                        // 暂停与空载时位置不动，也就不重绘
                        if got_events || moved {
                            cx.notify();
                        }
                        for event in emitted {
                            cx.emit(event);
                        }
                    })
                    .is_ok();

                // Err 说明窗口 / 实体已销毁，必须收工，否则任务泄漏
                if !alive {
                    break;
                }

                if disconnected {
                    break;
                }
            }
        });

        Self {
            handle,
            snapshot,
            state: PlaybackState::Idle,
            current: None,
            duration: Duration::ZERO,
            volume,
            error: startup_error,
            _pump: pump,
        }
    }

    /// 下发一条命令。音频线程起不来时静默忽略。
    fn send(&self, command: PlayerCommand) {
        if let Some(handle) = &self.handle {
            handle.send(command);
        }
    }

    /// 高频读取走无锁快照，不经过 Entity、不触发通知。
    pub fn position(&self) -> Duration {
        self.snapshot.position()
    }

    pub fn progress(&self) -> f32 {
        self.snapshot.progress()
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    pub fn state(&self) -> PlaybackState {
        self.state
    }

    pub fn is_playing(&self) -> bool {
        self.state.is_playing()
    }

    pub fn current(&self) -> Option<&Track> {
        self.current.as_ref()
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// 装载曲目：`current` 与状态乐观更新，时长稍后由 `PlayerEvent::Loaded` 校正。
    pub fn load(&mut self, track: Track, autoplay: bool, cx: &mut Context<Self>) {
        self.error = None;
        self.state = if autoplay {
            PlaybackState::Playing
        } else {
            PlaybackState::Paused
        };
        self.duration = track.duration.unwrap_or(Duration::ZERO);

        self.send(PlayerCommand::Load {
            path: track.path.clone(),
            autoplay,
        });
        self.current = Some(track);

        cx.emit(PlaybackEvent::TrackChanged);
        cx.notify();
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        self.send(PlayerCommand::TogglePlay);
        // 不做乐观更新：播放 / 暂停以音频线程回来的 State 事件为准
        cx.notify();
    }

    pub fn play(&mut self) {
        self.send(PlayerCommand::Play);
    }

    pub fn pause(&mut self) {
        self.send(PlayerCommand::Pause);
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.send(PlayerCommand::Stop);
        cx.notify();
    }

    /// 回到当前曲目的开头并继续播。
    pub fn restart(&mut self) {
        self.send(PlayerCommand::Seek(Duration::ZERO));
        self.send(PlayerCommand::Play);
    }

    /// 跳到指定位置。只在「用户松开进度条」时调用：rodio 的 seek 会重建解码器，拖动中连续调用会卡顿爆音。
    pub fn seek(&mut self, position: Duration) {
        self.send(PlayerCommand::Seek(position));
    }

    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        let clamped = volume.clamp(0.0, 1.0);
        self.volume = clamped;
        self.send(PlayerCommand::SetVolume(clamped));
        cx.emit(PlaybackEvent::VolumeChanged);
        cx.notify();
    }

    fn apply(&mut self, event: PlayerEvent, emitted: &mut Vec<PlaybackEvent>) {
        match event {
            PlayerEvent::Loaded { duration, .. } => {
                // 标签里的时长可能不准，以解码器报的为准
                if duration > Duration::ZERO {
                    self.duration = duration;
                }
            }

            PlayerEvent::State(state) => {
                if self.state != state {
                    self.state = state;
                    emitted.push(PlaybackEvent::StateChanged);
                }
            }

            PlayerEvent::Ended => {
                self.state = PlaybackState::Paused;
                emitted.push(PlaybackEvent::Finished);
            }

            PlayerEvent::Volume(volume) => {
                if self.volume != volume {
                    self.volume = volume;
                    emitted.push(PlaybackEvent::VolumeChanged);
                }
            }

            PlayerEvent::Error(message) => {
                self.error = Some(message);
                self.state = PlaybackState::Idle;
                emitted.push(PlaybackEvent::StateChanged);
            }
        }
    }
}
