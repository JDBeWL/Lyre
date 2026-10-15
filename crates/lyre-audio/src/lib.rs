//! 音频内核：cpal 的 `Stream` 不实现 `Send`，音频对象只能活在一条专属线程上。
//! UI 经 [`PlayerCommand`] / [`PlayerEvent`] 两条单向通道与它打交道，高频进度走 [`Snapshot`] 原子量。

pub mod backend;
pub mod command;
pub mod decode;
pub mod device;
mod device_monitor;
pub mod error;
pub mod event;
pub mod rodio_backend;
pub mod snapshot;
mod thread;

pub use backend::AudioBackend;
pub use command::PlayerCommand;
pub use decode::{LockFreeSource, SymphoniaDecoder, TrackDecoder, WhenEmpty};
pub use device::AudioDeviceInfo;
pub use error::{AudioError, Result};
pub use event::{OutputMode, PlaybackState, PlayerEvent};
pub use rodio_backend::RodioBackend;
pub use snapshot::Snapshot;

use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::device_monitor::DeviceMonitor;

/// 独占后端工厂，由上层注入 —— `lyre-audio-exclusive` 依赖本 crate 才能复用解码链，
/// 本 crate 不能反向依赖它。参数是目标设备的稳定标识，`None` 表示系统默认。
pub type ExclusiveOpenFn = Box<dyn FnMut(Option<&str>) -> Result<Box<dyn AudioBackend>> + Send>;

/// UI 侧持握的句柄，`Send + Clone`；真正的音频资源在它背后那条线程里。
#[derive(Clone)]
pub struct AudioHandle {
    tx: Sender<PlayerCommand>,
    snapshot: Arc<Snapshot>,
    /// 与句柄同寿命：最后一个句柄析构时设备监视线程随之收工。
    _monitor: Arc<DeviceMonitor>,
}

impl AudioHandle {
    /// 启动音频线程。默认使用 rodio 后端，`initial_device` 为 `None` 时跟随系统默认设备。
    pub fn spawn(events: Sender<PlayerEvent>, initial_device: Option<String>) -> Result<Self> {
        Self::spawn_inner(
            events,
            initial_device,
            Box::new(|device| {
                RodioBackend::open_with_device(device)
                    .map(|backend| Box::new(backend) as Box<dyn AudioBackend>)
            }),
            None,
        )
    }

    /// 启动音频线程，并带上独占后端工厂；没有它时 `SetOutputMode` 只会退回共享。
    pub fn spawn_with_exclusive(
        events: Sender<PlayerEvent>,
        initial_device: Option<String>,
        open_exclusive: ExclusiveOpenFn,
    ) -> Result<Self> {
        Self::spawn_inner(
            events,
            initial_device,
            Box::new(|device| {
                RodioBackend::open_with_device(device)
                    .map(|backend| Box::new(backend) as Box<dyn AudioBackend>)
            }),
            Some(open_exclusive),
        )
    }

    /// 启动音频线程，后端由调用方提供；懒创建 —— 第一条命令才开设备，没声卡也能启动。
    /// 返回 `Err` 只表示资源耗尽；设备打不开是懒创建阶段的事，经 [`PlayerEvent::Error`] 上报。
    pub fn spawn_with<F>(events: Sender<PlayerEvent>, open: F) -> Result<Self>
    where
        F: FnOnce() -> Result<Box<dyn AudioBackend>> + Send + 'static,
    {
        // 注入式后端只开一次；真实链路可以反复重开，这类工厂不行，重复调用按设备不可用上报。
        let mut once = Some(open);
        let reopen = move |_: Option<&str>| match once.take() {
            Some(open) => open(),
            None => Err(AudioError::DeviceOpen("音频设备不可用".to_owned())),
        };
        Self::spawn_inner(events, None, Box::new(reopen), None)
    }

    fn spawn_inner(
        events: Sender<PlayerEvent>,
        initial_device: Option<String>,
        open: thread::OpenFn,
        open_exclusive: Option<ExclusiveOpenFn>,
    ) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let (signals_tx, signals_rx) = std::sync::mpsc::channel();
        let snapshot = Arc::new(Snapshot::default());

        let thread_snapshot = Arc::clone(&snapshot);
        std::thread::Builder::new()
            .name("lyre-audio".to_owned())
            .spawn(move || {
                thread::run(
                    &rx,
                    &signals_rx,
                    events,
                    thread_snapshot,
                    initial_device,
                    open,
                    open_exclusive,
                );
            })
            .map_err(AudioError::ThreadSpawn)?;

        Ok(Self {
            tx,
            snapshot,
            _monitor: Arc::new(DeviceMonitor::start(signals_tx)),
        })
    }

    /// 下发一条命令。音频线程已经退出时静默忽略。
    pub fn send(&self, command: PlayerCommand) {
        let _ = self.tx.send(command);
    }

    /// 无锁快照。进度这种每帧都要读的数据走这里，不走事件队列。
    pub fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    pub fn position(&self) -> Duration {
        self.snapshot.position()
    }

    pub fn duration(&self) -> Duration {
        self.snapshot.duration()
    }

    pub fn volume(&self) -> f32 {
        self.snapshot.volume()
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(PlayerCommand::Shutdown);
    }
}
