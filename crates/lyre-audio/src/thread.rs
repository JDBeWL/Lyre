use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::backend::AudioBackend;
use crate::command::PlayerCommand;
use crate::error::{AudioError, Result};
use crate::event::{PlaybackState, PlayerEvent};
use crate::snapshot::Snapshot;

/// 轮询间隔，收命令与刷新进度都用它。50ms ≈ 进度条 20fps。
const TICK: Duration = Duration::from_millis(50);

type OpenFn = Box<dyn FnOnce() -> Result<Box<dyn AudioBackend>>>;

pub(crate) fn run(
    rx: &Receiver<PlayerCommand>,
    events: Sender<PlayerEvent>,
    snapshot: Arc<Snapshot>,
    open: impl FnOnce() -> Result<Box<dyn AudioBackend>> + 'static,
) {
    let mut thread = AudioThread {
        events,
        snapshot,
        open: Some(Box::new(open)),
        backend: None,
        seen_content: false,
        ended_reported: false,
    };

    loop {
        match rx.recv_timeout(TICK) {
            // 收到 Shutdown，或 UI 侧的句柄全部 drop 了 —— 两种都是收工
            Ok(PlayerCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(command) => thread.handle(command),
            Err(RecvTimeoutError::Timeout) => {}
        }
        thread.tick();
    }
}

/// 音频线程状态。`!Send`（持有后端），永远不离开它所在那条线程。
struct AudioThread {
    events: Sender<PlayerEvent>,
    snapshot: Arc<Snapshot>,
    /// 懒创建：真的收到命令时才去开设备。失败后不再重试（置 `None`）。
    open: Option<OpenFn>,
    backend: Option<Box<dyn AudioBackend>>,
    /// 本曲是否已经真正出声过 —— 用来区分「刚开始装载」和「播完了」
    seen_content: bool,
    /// 本轮播放是否已经上报过 Ended，避免重复上报
    ended_reported: bool,
}

impl AudioThread {
    fn backend(&mut self) -> Option<&mut dyn AudioBackend> {
        if self.backend.is_none() {
            let open = self.open.take()?;
            match open() {
                Ok(backend) => self.backend = Some(backend),
                Err(err) => {
                    self.snapshot.set_state(PlaybackState::Idle);
                    let _ = self.events.send(PlayerEvent::Error(err.to_string()));
                    return None;
                }
            }
        }
        self.backend.as_deref_mut()
    }

    fn emit_state(&self) {
        let _ = self.events.send(PlayerEvent::State(self.snapshot.state()));
    }

    fn handle(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::Shutdown => {}

            PlayerCommand::Load { path, autoplay } => {
                let outcome = self.backend().map(|backend| backend.load(&path, autoplay));

                let Some(outcome) = outcome else {
                    // 设备打不开过一次之后 `backend()` 不再重试。这里必须抛出异常，
                    // 否则 `PlaybackEntity::load` 乐观设成的播放状态会导致播放出错。
                    let _ = self.events.send(PlayerEvent::Error(
                        AudioError::DeviceOpen("音频设备不可用".to_owned()).to_string(),
                    ));
                    return;
                };

                match outcome {
                    Ok(duration) => {
                        self.seen_content = false;
                        self.ended_reported = false;
                        self.snapshot.set_position(Duration::ZERO);
                        self.snapshot.set_duration(duration);
                        self.snapshot.set_state(if autoplay {
                            PlaybackState::Playing
                        } else {
                            PlaybackState::Paused
                        });
                        let _ = self.events.send(PlayerEvent::Loaded { path, duration });
                        self.emit_state();
                    }
                    Err(err) => {
                        self.snapshot.set_state(PlaybackState::Idle);
                        let _ = self.events.send(PlayerEvent::Error(err.to_string()));
                        self.emit_state();
                    }
                }
            }

            PlayerCommand::Play => {
                match self.backend().map(AudioBackend::play) {
                    Some(Ok(())) => {
                        // 刚 append 的音源在头几毫秒里 empty() 仍为 true，两个标记都得清掉，
                        // 否则 tick 会立刻误报 Ended
                        self.ended_reported = false;
                        self.seen_content = false;
                        self.snapshot.set_state(PlaybackState::Playing);
                        self.emit_state();
                    }
                    // 音源重建失败：退回 Idle，界面不该在静音时显示「正在播放」
                    Some(Err(err)) => {
                        self.snapshot.set_state(PlaybackState::Idle);
                        let _ = self.events.send(PlayerEvent::Error(err.to_string()));
                        self.emit_state();
                    }
                    // 设备没打开过就什么都不做
                    None => {}
                }
            }

            PlayerCommand::Pause => {
                let ok = self.backend().map(AudioBackend::pause).is_some();
                if ok {
                    self.snapshot.set_state(PlaybackState::Paused);
                    self.emit_state();
                }
            }

            PlayerCommand::TogglePlay => {
                let next = if self.snapshot.state().is_playing() {
                    PlayerCommand::Pause
                } else {
                    PlayerCommand::Play
                };
                self.handle(next);
            }

            PlayerCommand::Stop => {
                let ok = self.backend().map(AudioBackend::stop).is_some();
                if ok {
                    self.seen_content = false;
                    self.ended_reported = true;
                    self.snapshot.set_position(Duration::ZERO);
                    self.snapshot.set_state(PlaybackState::Stopped);
                    self.emit_state();
                }
            }

            PlayerCommand::Seek(position) => {
                let result = self.backend().map(|b| b.seek(position));
                match result {
                    Some(Ok(())) => {
                        // 乐观更新：不等下一次 tick，进度条立刻跟手
                        self.snapshot.set_position(position);
                        self.ended_reported = false;
                    }
                    // 失败必须抛出异常，否则进度条自己弹回去，界面上看不出任何原因。
                    Some(Err(err)) => {
                        let _ = self.events.send(PlayerEvent::Error(err.to_string()));
                    }
                    // 设备没打开过还是什么都不做
                    None => {}
                }
            }

            PlayerCommand::SetVolume(volume) => {
                let clamped = volume.clamp(0.0, 1.0);
                let ok = self.backend().map(|b| b.set_volume(clamped)).is_some();
                if ok {
                    self.snapshot.set_volume(clamped);
                    let _ = self.events.send(PlayerEvent::Volume(clamped));
                }
            }
        }
    }

    /// 每 50ms 一次刷新进度快照，并检测自然播放结束。
    fn tick(&mut self) {
        let probe = self
            .backend
            .as_deref_mut()
            .map(|backend| (backend.position(), backend.is_finished()));

        let Some((position, finished)) = probe else {
            return;
        };

        self.snapshot.set_position(position);

        if !finished {
            // 真的流淌过音频数据了，之后 empty() 才是真的 empty
            self.seen_content = true;
            return;
        }

        // 刚 append 的头几十毫秒里 empty() 仍可能为 true，必须先真正出过声再判定结束
        if self.seen_content
            && !self.ended_reported
            && self.snapshot.state() == PlaybackState::Playing
        {
            self.ended_reported = true;
            self.snapshot.set_state(PlaybackState::Paused);
            let _ = self.events.send(PlayerEvent::Ended);
            self.emit_state();
        }
    }
}
