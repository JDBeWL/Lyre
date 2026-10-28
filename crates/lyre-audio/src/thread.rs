use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::ExclusiveOpenFn;
use crate::backend::{AudioBackend, BackendNotice};
use crate::command::PlayerCommand;
use crate::device::{self, AudioDeviceInfo};
use crate::device_monitor::DeviceSignal;
use crate::error::{AudioError, Result};
use crate::event::{OutputMode, PlaybackState, PlayerEvent};
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
    open_exclusive: Option<ExclusiveOpenFn>,
) {
    let mut thread = AudioThread {
        events,
        snapshot,
        open,
        open_exclusive,
        output_mode: OutputMode::Shared,
        current: None,
        // 启动时还没开过流，把手上的选择当作已生效的目标，免得第一条命令就白切一次
        device: initial_device.clone(),
        last_target: initial_device,
        default_id: device::default_device_id(),
        backend: None,
        open_failed: false,
        seen_content: false,
        ended_reported: false,
        smooth_switch: false,
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
    /// 独占后端的工厂；`None` 表示本次运行没有可用的独占后端。
    open_exclusive: Option<ExclusiveOpenFn>,
    /// 当前生效的输出模式。
    output_mode: OutputMode,
    /// 最近一次成功装载的曲目。切输出模式之后要用它把播放接回原位置 ——
    /// 位置与播放状态本身从快照读，不另存一份，免得两份状态各自漂移。
    current: Option<PathBuf>,
    /// 用户选定的输出设备；`None` 表示跟随系统默认，拔掉时不清空。
    /// 这是用户意图，插回来要自动切回；设备不在时由 [`AudioThread::target_now`] 用系统默认顶上。
    device: Option<String>,
    /// 上一次真正开流用的目标。设备插回来时它和当前目标会不一样，据此判断要不要切回去。
    last_target: Option<String>,
    /// 上一次看到的系统默认设备，用来判断「跟随默认」时是否需要重建输出流。
    default_id: Option<String>,
    /// 还没打开过时是 `None`。
    backend: Option<Box<dyn AudioBackend>>,
    /// 打开失败后置真，避免后台拍子反复重试开设备刷屏；
    /// 用户显式播放 / 装载 / 切设备时复位。
    open_failed: bool,
    /// 本曲是否已经真正出声过 —— 用来区分「刚开始装载」和「播完了」
    seen_content: bool,
    /// 本轮播放是否已经上报过 Ended，避免重复上报
    ended_reported: bool,
    /// 换设备时是否交叉淡化。后端是懒创建的，所以标志得先在控制线程里存着。
    smooth_switch: bool,
}

impl AudioThread {
    /// 懒创建：真的收到命令时才去开设备。
    fn backend(&mut self) -> Option<&mut dyn AudioBackend> {
        if self.backend.is_none() {
            if self.open_failed {
                return None;
            }
            match self.open_backend() {
                Ok((mut backend, mode, detail)) => {
                    backend.set_smooth_switch(self.smooth_switch);
                    // 音量可能在设备打开之前就设过了，新后端得补上，否则会是默认的满音量
                    backend.set_volume(self.snapshot.volume());
                    self.backend = Some(backend);
                    self.output_mode = mode;
                    let _ = self
                        .events
                        .send(PlayerEvent::OutputModeChanged { mode, detail });
                }
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

    /// 按当前输出模式开一条流。要独占却没开起来时退回共享，并额外发一条错误而不是静默降级。
    fn open_backend(&mut self) -> Result<(Box<dyn AudioBackend>, OutputMode, String)> {
        let device = self.target_now();
        let events = self.events.clone();

        if self.output_mode == OutputMode::Exclusive {
            let opened = self
                .open_exclusive
                .as_mut()
                .map(|factory| factory(device.as_deref()));
            match opened {
                Some(Ok(backend)) => {
                    let detail = backend.describe_output();
                    return Ok((backend, OutputMode::Exclusive, detail));
                }
                Some(Err(err)) => {
                    log::warn!("独占输出不可用，退回共享输出：{err}");
                    let _ = events.send(PlayerEvent::Error(err.to_string()));
                    self.output_mode = OutputMode::Shared;
                    let _ = events.send(PlayerEvent::OutputModeChanged {
                        mode: OutputMode::Shared,
                        detail: err.to_string(),
                    });
                }
                // 本次运行没有注入独占后端，直接走共享
                None => {}
            }
        }

        let backend = (self.open)(device.as_deref())?;
        Ok((backend, OutputMode::Shared, String::new()))
    }

    /// 切输出模式。独占与共享在同一台设备上互斥，所以先把手上这条流停掉、放掉设备，
    /// 再按新模式开流，最后把当前曲目接回原来的位置。
    fn select_output_mode(&mut self, exclusive: bool) {
        let wanted = if exclusive {
            OutputMode::Exclusive
        } else {
            OutputMode::Shared
        };
        if wanted == self.output_mode && self.backend.is_some() {
            return;
        }

        if wanted == OutputMode::Exclusive && self.open_exclusive.is_none() {
            // 本次运行没有注入独占后端。不动正在播的那条流，但也不能让开关看起来生效了。
            let _ = self.events.send(PlayerEvent::Error(
                AudioError::Exclusive("本次运行没有可用的独占后端".to_owned()).to_string(),
            ));
            let _ = self.events.send(PlayerEvent::OutputModeChanged {
                mode: OutputMode::Shared,
                detail: String::new(),
            });
            return;
        }

        if let Some(backend) = self.backend.as_deref_mut() {
            backend.stop();
        }
        self.backend = None;
        self.open_failed = false;
        self.output_mode = wanted;
        let _ = self.events.send(PlayerEvent::OutputModeChanged {
            mode: wanted,
            detail: String::new(),
        });

        // 后端是懒创建的，这里不主动开流；但正在播的曲目必须接回原位置
        self.restore_after_switch();
    }

    /// 换完输出流之后把当前曲目接回原来的位置与播放状态。
    fn restore_after_switch(&mut self) {
        let Some(path) = self.current.clone() else {
            return;
        };
        let state = self.snapshot.state();
        if matches!(state, PlaybackState::Idle | PlaybackState::Stopped) {
            return;
        }

        let position = self.snapshot.position();
        let duration = self.snapshot.duration();
        let playing = state.is_playing();

        let outcome = self.backend().map(|backend| {
            backend.load(&path, false).and_then(|_| {
                if position.is_zero() {
                    Ok(())
                } else {
                    backend.seek(position)
                }
            })
        });

        match outcome {
            Some(Ok(())) => {
                self.seen_content = false;
                self.ended_reported = false;
                self.snapshot.set_position(position);
                self.snapshot.set_duration(duration);

                if playing && let Some(backend) = self.backend.as_deref_mut() {
                    let _ = backend.play();
                }
                let volume = self.snapshot.volume();
                if let Some(backend) = self.backend.as_deref_mut() {
                    backend.set_volume(volume);
                }

                self.snapshot.set_state(if playing {
                    PlaybackState::Playing
                } else {
                    PlaybackState::Paused
                });
                self.emit_state();
                self.announce_output();
            }
            Some(Err(err)) => {
                self.snapshot.set_state(PlaybackState::Idle);
                let _ = self.events.send(PlayerEvent::Error(err.to_string()));
                self.emit_state();
            }
            None => {}
        }
    }

    fn emit_state(&self) {
        let _ = self.events.send(PlayerEvent::State(self.snapshot.state()));
    }

    /// 把后端实际用的输出口径播报出去。独占口径会随曲目变（要按声源重开流），
    /// 只在创建后端时报一次的话，界面会停在第一首协商到的口径上。
    fn announce_output(&self) {
        let Some(backend) = self.backend.as_deref() else {
            return;
        };
        let _ = self.events.send(PlayerEvent::OutputModeChanged {
            mode: self.output_mode,
            detail: backend.describe_output(),
        });
    }

    /// 当前真正生效的设备：优先信后端（它知道流实际落在哪个设备上，选定设备被拔掉时
    /// 它会顶在默认设备上），后端还没建起来时才轮到用户的选择。
    fn effective_device(&self, devices: &[AudioDeviceInfo]) -> (Option<String>, String) {
        let selected = match self.backend.as_deref() {
            Some(backend) => backend.current_device(),
            None => Self::target_of(self.device.as_deref(), devices),
        };

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

    /// 现在该往哪台设备开流。选定的设备还在就用它；没选、或者选的那台被拔掉了，
    /// 就用系统默认顶着 —— 拔掉期间不能没声音，插回来还要自动切回去。
    fn target_now(&self) -> Option<String> {
        Self::target_of(self.device.as_deref(), &device::enumerate_output_devices())
    }

    /// 从设备列表里挑出该用的那台：选定设备还在列表里就是它，否则是 `None`（用系统默认）。
    fn target_of(selected: Option<&str>, devices: &[AudioDeviceInfo]) -> Option<String> {
        let id = selected?;
        devices
            .iter()
            .any(|device| device.id == id)
            .then(|| id.to_owned())
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
                self.announce_output();
                self.last_target = id.map(str::to_owned);
            }
            Some(Err(err)) => {
                // 后端会回滚到旧设备接着播，记忆的目标也必须留在旧值上
                let _ = self.events.send(PlayerEvent::Error(err.to_string()));
            }
            // 后端还没建起来：下次懒创建自然用新设备，此刻没有音源需要重置
            None => self.last_target = id.map(str::to_owned),
        }
    }

    fn select_device(&mut self, id: Option<String>) {
        self.device = id;
        // 换了目标设备就该再给一次机会：上一次的失败是针对旧设备的
        self.open_failed = false;

        let target = self.target_now();
        self.switch_output(target.as_deref());

        self.refresh_devices();
    }

    /// 设备列表变了。三件事都要处理：选定的设备被拔掉（先拿默认顶着）、跟随默认时默认设备换了、
    /// 以及选定的设备插回来了（切回去）。cpal 的流不会自己迁移到别的设备上，不重建就一直没声。
    fn handle_topology_changed(&mut self) {
        let devices = device::enumerate_output_devices();
        let default_id = devices
            .iter()
            .find(|device| device.is_default)
            .map(|device| device.id.clone());

        let target = Self::target_of(self.device.as_deref(), &devices);
        if self.device.is_some() && target.is_none() {
            log::info!("选定的输出设备已断开，暂用系统默认设备顶上");
        }

        // 跟随默认时目标恒为「默认」，所以默认设备换掉要多判一次；其余情况看目标本身有没有变
        let follows_default = self.device.is_none() && self.default_id != default_id;
        let target_moved = self.last_target != target;
        self.default_id = default_id;

        if target_moved || follows_default {
            self.open_failed = false;
            self.switch_output(target.as_deref());
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

            PlayerCommand::SetOutputMode { exclusive } => self.select_output_mode(exclusive),

            PlayerCommand::SetSmoothSwitch(enabled) => {
                self.smooth_switch = enabled;
                if let Some(backend) = self.backend.as_deref_mut() {
                    backend.set_smooth_switch(enabled);
                }
            }

            PlayerCommand::Load { path, autoplay } => {
                // 用户显式装载：上一次打不开不代表这次也不行，必须先给设备一次机会
                self.open_failed = false;
                let outcome = self.backend().map(|backend| backend.load(&path, autoplay));

                let Some(outcome) = outcome else {
                    // `backend()` 已经上报了具体原因（并把快照退回 Idle），这里只把状态同步给界面，
                    // 否则 `PlaybackEntity::load` 乐观设成的播放状态会一直挂着
                    self.emit_state();
                    return;
                };

                match outcome {
                    Ok(duration) => {
                        self.current = Some(path.clone());
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
                        // 独占会按这一首的口径重开流，协商结果要跟上，否则界面显示的还是上一首的
                        self.announce_output();
                    }
                    Err(err) => {
                        self.snapshot.set_state(PlaybackState::Idle);
                        let _ = self.events.send(PlayerEvent::Error(err.to_string()));
                        self.emit_state();
                    }
                }
            }

            PlayerCommand::Play => {
                // 显式播放是用户对「设备现在能用了」的重试机会
                self.open_failed = false;
                match self.backend().map(AudioBackend::play) {
                    Some(Ok(())) => {
                        // 刚 append 的音源在头几毫秒里 empty() 仍为 true，两个标记都得清掉，
                        // 否则 tick 会立刻误报 Ended
                        self.ended_reported = false;
                        self.seen_content = false;
                        self.snapshot.set_state(PlaybackState::Playing);
                        self.emit_state();
                    }
                    // 重建失败：退回 Idle，界面不该在静音时显示「正在播放」
                    Some(Err(err)) => {
                        self.snapshot.set_state(PlaybackState::Idle);
                        let _ = self.events.send(PlayerEvent::Error(err.to_string()));
                        self.emit_state();
                    }
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
                    None => {}
                }
            }

            PlayerCommand::SetVolume(volume) => {
                let clamped = volume.clamp(0.0, 1.0);
                self.snapshot.set_volume(clamped);
                // 不为调音量开输出流：启动时下发的存档音量是最早到的一条命令，
                // 开流会让音量合成器留下会话；设备留到装载时再碰，音量由 `backend()` 补发。
                if let Some(backend) = self.backend.as_deref_mut() {
                    backend.set_volume(clamped);
                }
                let _ = self.events.send(PlayerEvent::Volume(clamped));
            }
        }
    }

    /// 每 50ms 一次刷新进度快照，并检测自然播放结束。
    fn tick(&mut self) {
        let Some(backend) = self.backend.as_deref_mut() else {
            return;
        };
        // 淡出、挂起的交接这类收尾要等时间，靠这一拍推着走；设备失联的恢复也在这里推进
        backend.tick();
        let notices: Vec<BackendNotice> = std::iter::from_fn(|| backend.take_notice()).collect();
        let (position, finished) = (backend.position(), backend.is_finished());

        for notice in notices {
            match notice {
                BackendNotice::Lost(err) => {
                    let _ = self.events.send(PlayerEvent::Error(err.to_string()));
                    if self.snapshot.state().is_playing() {
                        self.snapshot.set_state(PlaybackState::Paused);
                    }
                    // 界面收到 Error 会先把状态显示成 Idle；这里把音频线程的真实状态补发一次
                    self.emit_state();
                    // 流已经断了，不能再把下一拍的空缓冲判成自然播完
                    self.seen_content = false;
                    self.ended_reported = true;
                }
                BackendNotice::Resumed => {
                    // 后端从断流里自己恢复了：状态和输出口径都重新播报一遍
                    self.seen_content = false;
                    self.ended_reported = false;
                    self.snapshot.set_state(PlaybackState::Playing);
                    self.emit_state();
                    self.announce_output();
                }
            }
        }

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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use super::{AudioThread, OpenFn};
    use crate::backend::{AudioBackend, BackendNotice};
    use crate::command::PlayerCommand;
    use crate::device::AudioDeviceInfo;
    use crate::error::{AudioError, Result};
    use crate::event::{OutputMode, PlaybackState, PlayerEvent};
    use crate::snapshot::Snapshot;

    struct NullBackend;

    impl AudioBackend for NullBackend {
        fn load(&mut self, _path: &Path, _autoplay: bool) -> Result<Duration> {
            Ok(Duration::from_secs(60))
        }

        fn play(&mut self) -> Result<()> {
            Ok(())
        }

        fn pause(&mut self) {}

        fn stop(&mut self) {}

        fn seek(&mut self, _position: Duration) -> Result<()> {
            Ok(())
        }

        fn set_volume(&mut self, _volume: f32) {}

        fn position(&self) -> Duration {
            Duration::ZERO
        }

        fn is_finished(&self) -> bool {
            false
        }

        fn set_device(&mut self, _device: Option<&str>) -> Result<()> {
            Err(AudioError::DeviceOpen("switch failed".to_owned()))
        }
    }

    /// 由用例直接塞通知的后端；`is_finished` 恒真，用来验证断流不会被误判成播完。
    struct NoticeBackend {
        notices: Arc<Mutex<VecDeque<BackendNotice>>>,
    }

    impl AudioBackend for NoticeBackend {
        fn load(&mut self, _path: &Path, _autoplay: bool) -> Result<Duration> {
            Ok(Duration::ZERO)
        }

        fn play(&mut self) -> Result<()> {
            Ok(())
        }

        fn pause(&mut self) {}

        fn stop(&mut self) {}

        fn seek(&mut self, _position: Duration) -> Result<()> {
            Ok(())
        }

        fn set_volume(&mut self, _volume: f32) {}

        fn position(&self) -> Duration {
            Duration::from_secs(5)
        }

        fn is_finished(&self) -> bool {
            true
        }

        fn take_notice(&mut self) -> Option<BackendNotice> {
            self.notices.lock().expect("notices lock").pop_front()
        }
    }

    fn thread(open: OpenFn) -> (AudioThread, mpsc::Receiver<PlayerEvent>, Arc<Snapshot>) {
        let (events, events_rx) = mpsc::channel();
        let snapshot = Arc::new(Snapshot::default());
        let thread = AudioThread {
            events,
            snapshot: Arc::clone(&snapshot),
            open,
            open_exclusive: None,
            output_mode: OutputMode::Shared,
            current: None,
            device: None,
            last_target: None,
            default_id: None,
            backend: None,
            open_failed: false,
            seen_content: false,
            ended_reported: false,
            smooth_switch: false,
        };
        (thread, events_rx, snapshot)
    }

    #[test]
    fn an_explicit_play_retries_a_device_that_failed_to_open() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let opened = Arc::clone(&attempts);
        let (mut thread, events, snapshot) = thread(Box::new(move |_| {
            if opened.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(AudioError::DeviceOpen("device busy".to_owned()))
            } else {
                Ok(Box::new(NullBackend) as Box<dyn AudioBackend>)
            }
        }));

        thread.handle(PlayerCommand::Play);
        assert_eq!(
            snapshot.state(),
            PlaybackState::Idle,
            "第一次打不开只能退回 Idle"
        );
        assert!(
            matches!(events.try_recv(), Ok(PlayerEvent::Error(_))),
            "第一次失败要上报"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        // 设备可能只是被别的程序临时占着：用户再点一次播放必须真的重试，而不是被 open_failed 挡住
        thread.handle(PlayerCommand::Play);
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "显式播放必须重新尝试开设备"
        );
        assert_eq!(
            snapshot.state(),
            PlaybackState::Playing,
            "设备恢复后应当能播"
        );
    }

    #[test]
    fn a_failed_device_switch_does_not_move_the_remembered_target() {
        let (mut thread, _events, _snapshot) = thread(Box::new(|_| {
            Ok(Box::new(NullBackend) as Box<dyn AudioBackend>)
        }));
        thread.handle(PlayerCommand::Play);
        thread.last_target = Some("old".to_owned());

        thread.handle(PlayerCommand::SetDevice { id: None });

        assert_eq!(
            thread.last_target.as_deref(),
            Some("old"),
            "切换失败时后端已经回滚到旧设备，记忆的目标不能跟着变"
        );
    }

    #[test]
    fn a_lost_stream_parks_playback_until_the_backend_resumes() {
        let notices = Arc::new(Mutex::new(VecDeque::new()));
        let (mut thread, events, snapshot) = thread(Box::new(|_| {
            Err(AudioError::DeviceOpen("这条用例不靠工厂开口".to_owned()))
        }));
        thread.backend = Some(Box::new(NoticeBackend {
            notices: Arc::clone(&notices),
        }));
        snapshot.set_state(PlaybackState::Playing);

        notices
            .lock()
            .expect("notices lock")
            .push_back(BackendNotice::Lost(AudioError::Exclusive(
                "独占输出中断：测试".to_owned(),
            )));
        thread.tick();

        assert_eq!(
            snapshot.state(),
            PlaybackState::Paused,
            "流断了就不能继续显示播放中"
        );
        assert!(snapshot.position() > Duration::ZERO, "位置不能被打回零");
        assert!(matches!(events.try_recv(), Ok(PlayerEvent::Error(_))));
        assert!(matches!(
            events.try_recv(),
            Ok(PlayerEvent::State(PlaybackState::Paused))
        ));
        // 后端此刻报 is_finished，但流是断的，不能顺手报 Ended
        assert!(matches!(events.try_recv(), Err(mpsc::TryRecvError::Empty)));

        notices
            .lock()
            .expect("notices lock")
            .push_back(BackendNotice::Resumed);
        thread.tick();

        assert_eq!(
            snapshot.state(),
            PlaybackState::Playing,
            "后端自行重建并续播之后，状态要跟回来"
        );
    }

    fn device(id: &str, is_default: bool) -> AudioDeviceInfo {
        AudioDeviceInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            is_default,
        }
    }

    #[test]
    fn a_selected_device_that_is_gone_falls_back_to_the_default() {
        let devices = [device("speakers", true), device("headset", false)];

        assert_eq!(
            AudioThread::target_of(Some("headset"), &devices).as_deref(),
            Some("headset"),
            "选中的设备还在就用它"
        );
        assert_eq!(
            AudioThread::target_of(Some("usb-dac"), &devices),
            None,
            "选中的被拔掉了，该交给系统默认顶着"
        );
        assert_eq!(
            AudioThread::target_of(None, &devices),
            None,
            "没选就是跟随默认"
        );

        // 用户的选择不因拔插消失：设备回到列表里，目标又会指向它 —— 这就是「插回来切回去」的依据
        let reattached = [device("speakers", true), device("usb-dac", false)];
        assert_eq!(
            AudioThread::target_of(Some("usb-dac"), &reattached).as_deref(),
            Some("usb-dac")
        );
    }
}
