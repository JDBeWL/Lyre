//! 音频内核：cpal 的 `Stream` 不实现 `Send`，音频对象只能活在一条专属线程上。
//! UI 经 [`PlayerCommand`] / [`PlayerEvent`] 两条单向通道与它打交道，高频进度走 [`Snapshot`] 原子量。

pub mod backend;
pub mod command;
pub mod decode;
pub mod error;
pub mod event;
pub mod rodio_backend;
pub mod snapshot;
mod thread;

pub use backend::AudioBackend;
pub use command::PlayerCommand;
pub use decode::{LockFreeSource, SymphoniaDecoder};
pub use error::{AudioError, Result};
pub use event::{PlaybackState, PlayerEvent};
pub use rodio_backend::RodioBackend;
pub use snapshot::Snapshot;

use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// UI 侧持握的句柄，`Send + Clone`；真正的音频资源在它背后那条线程里。
#[derive(Clone)]
pub struct AudioHandle {
    tx: Sender<PlayerCommand>,
    snapshot: Arc<Snapshot>,
}

impl AudioHandle {
    /// 启动音频线程。默认使用 rodio 后端。
    pub fn spawn(events: Sender<PlayerEvent>) -> Result<Self> {
        Self::spawn_with(events, || {
            RodioBackend::open().map(|b| Box::new(b) as Box<dyn AudioBackend>)
        })
    }

    /// 启动音频线程，后端由调用方提供；懒创建 —— 第一条命令才开设备，没声卡也能启动。
    ///
    /// 返回 `Err` 只表示资源耗尽，设备打不开是懒创建阶段的事，
    /// 会经 [`PlayerEvent::Error`] 上报，不是这里的错误。
    pub fn spawn_with<F>(events: Sender<PlayerEvent>, open: F) -> Result<Self>
    where
        F: FnOnce() -> Result<Box<dyn AudioBackend>> + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let snapshot = Arc::new(Snapshot::default());

        let thread_snapshot = Arc::clone(&snapshot);
        std::thread::Builder::new()
            .name("lyre-audio".to_owned())
            .spawn(move || thread::run(&rx, events, thread_snapshot, open))
            .map_err(AudioError::ThreadSpawn)?;

        Ok(Self { tx, snapshot })
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
