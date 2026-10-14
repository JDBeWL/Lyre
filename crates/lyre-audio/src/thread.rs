use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::backend::AudioBackend;
use crate::command::PlayerCommand;
use crate::device::{self, AudioDeviceInfo};
use crate::device_monitor::DeviceSignal;
use crate::error::{AudioError, Result};
use crate::event::{PlaybackState, PlayerEvent};
use crate::snapshot::Snapshot;

/// 轮询间隔，收命令与刷新进度都用它。50ms ≈ 进度条 20fps。
const TICK: Duration = Duration::from_millis(50);

/// 后端工厂，参数是目标设备标识（`None` 为跟随系统默认）。
/// 可以重复调用：切换设备、选定设备被拔掉都要重开一条输出流。
pub(crate) type OpenFn = Box<dyn FnMut(Option<&str>) -> Result<Box<dyn AudioBackend>> + Send>;

pub(crate) fn run(
    rx: &Receiver<PlayerCommand>,
    device_signals: &Receiver<DeviceSignal>,
    events: Sender<PlayerEvent>,
    snapshot: Arc<Snapshot>,
    initial_device: Option<String>,
    open: OpenFn,
) {
    let mut thread = AudioThread {
        events,
        snapshot,
        open,
        device: initial_device,
        default_id: device::default_device_id(),
        backend: None,
        open_failed: false,
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
        thread.drain_device_signals(device_signals);
        thread.tick();
    }
}

/// 音频线程状态。`!Send`（持有后端），永远不离开它所在那条线程。
struct AudioThread {
    events: Sender<PlayerEvent>,
    snapshot: Arc<Snapshot>,
    open: OpenFn,
    /// 用户选定的输出设备；`None` 表示跟随系统默认。
    device: Option<String>,
    /// 上一次看到的系统默认设备，用来判断「跟随默认」时是否需要重建输出流。
    default_id: Option<String>,
    /// 还没打开过时是 `None`。
    backend: Option<Box<dyn AudioBackend>>,
    /// 打开失败后置真，避免每条命令都重试开设备刷屏；切换设备时复位。
    open_failed: bool,
    /// 本曲是否已经真正出声过 —— 用来区分「刚开始装载」和「播完了」
    seen_content: bool,
    /// 本轮播放是否已经上报过 Ended，避免重复上报
    ended_reported: bool,
}

impl AudioThread {
    /// 懒创建：真的收到命令时才去开设备。
    fn backend(&mut self) -> Option<&mut dyn AudioBackend> {
        if self.backend.is_none() {
            if self.open_failed {
                return None;
            }
            let device = self.device.clone();
            match (self.open)(device.as_deref()) {
                Ok(backend) => self.backend = Some(backend),
                Err(err) => {
                    self.open_failed = true;
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

    /// 当前真正生效的设备：优先信后端（它知道标识解析后落到了哪个设备），
    /// 其次是用户的选择，都没有就是系统默认。
    fn effective_device(&self, devices: &[AudioDeviceInfo]) -> (Option<String>, String) {
        let selected = self
            .backend
            .as_deref()
            .and_then(AudioBackend::current_device)
            .or_else(|| self.device.clone());

        let Some(id) = selected else {
            let name = devices
                .iter()
                .find(|device| device.is_default)
                .map_or_else(String::new, |device| device.name.clone());
            return (None, name);
        };

        let name = devices
            .iter()
            .find(|device| device.id == id)
            .map_or_else(|| id.clone(), |device| device.name.clone());
        (Some(id), name)
    }

    /// 重新枚举设备，并把「当前设备是谁」广播出去。
    fn refresh_devices(&mut self) {
        let devices = device::enumerate_output_devices();
        self.default_id = devices
            .iter()
            .find(|device| device.is_default)
            .map(|device| device.id.clone());

        let (id, name) = self.effective_device(&devices);
        let _ = self.events.send(PlayerEvent::Devices(devices));
        let _ = self.events.send(PlayerEvent::DeviceChanged { id, name });
    }

    /// 把后端切到目标设备。换流之后新音源在头几毫秒里 `empty()` 仍可能为 true，所以要和 [`Self::handle`]
    /// 里 `Play` 那样清掉两个结束判定标记，否则 `tick` 会误报 Ended，界面当场跳下一曲。
    fn switch_output(&mut self, id: Option<&str>) {
        let outcome = self
            .backend
            .as_deref_mut()
            .map(|backend| backend.set_device(id));
        match outcome {
            Some(Ok(())) => {
                self.seen_content = false;
                self.ended_reported = false;
            }
            Some(Err(err)) => {
                let _ = self.events.send(PlayerEvent::Error(err.to_string()));
            }
            // 后端还没建起来：下次懒创建自然用新设备，此刻没有音源需要重置
            None => {}
        }
    }

    fn select_device(&mut self, id: Option<String>) {
        self.device = id;
        // 换了目标设备就该再给一次机会：上一次的失败是针对旧设备的
        self.open_failed = false;

        let selected = self.device.clone();
        self.switch_output(selected.as_deref());

        self.refresh_devices();
    }

    /// 设备列表变了。选定设备被拔掉就回退跟随默认；跟随默认时默认设备换了也要重建输出流 ——
    /// cpal 的流不会自己迁移到新设备上，不重建就一直没声。
    fn handle_topology_changed(&mut self) {
        let devices = device::enumerate_output_devices();
        let default_id = devices
            .iter()
            .find(|device| device.is_default)
            .map(|device| device.id.clone());

        let selected_gone = self
            .device
            .as_ref()
            .is_some_and(|id| !devices.iter().any(|device| &device.id == id));
        if selected_gone {
            self.device = None;
            self.open_failed = false;
            log::info!("选定的输出设备已断开，改为跟随系统默认设备");
        }

        let follows_default = self.device.is_none() && self.default_id != default_id;
        self.default_id = default_id;

        if selected_gone || follows_default {
            let selected = self.device.clone();
            self.switch_output(selected.as_deref());
        }

        self.refresh_devices();
    }

    fn drain_device_signals(&mut self, signals: &Receiver<DeviceSignal>) {
        let mut changed = false;
        while signals.try_recv().is_ok() {
            changed = true;
        }
        if changed {
            self.handle_topology_changed();
        }
    }

    fn handle(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::Shutdown => {}

            PlayerCommand::EnumerateDevices => {
                let _ = self
                    .events
                    .send(PlayerEvent::Devices(device::enumerate_output_devices()));
            }

            PlayerCommand::SetDevice { id } => self.select_device(id),

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
