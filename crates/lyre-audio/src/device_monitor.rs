//! 输出设备的插拔与默认设备变化监听。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::device;

/// 轮询周期。设备插拔不是高频事件，500ms 足够跟手。
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// 轮询切成小片，退出时不必等满一个周期。
const POLL_SLICE: Duration = Duration::from_millis(50);
const SLICES: u32 = (POLL_INTERVAL.as_millis() / POLL_SLICE.as_millis()) as u32;

/// 监视线程发给控制线程的信号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceSignal {
    /// 设备列表或系统默认设备发生了变化。
    TopologyChanged,
}

/// 后台轮询设备列表，变化时通知控制线程。随最后一个 `AudioHandle` 一起析构。
pub(crate) struct DeviceMonitor {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl DeviceMonitor {
    pub(crate) fn start(signals: Sender<DeviceSignal>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        // 起不来线程只是失去热插拔跟随，不该让整个音频链路失败
        let handle = std::thread::Builder::new()
            .name("device-monitor".to_owned())
            .spawn(move || poll_loop(&signals, &thread_stop))
            .ok();

        Self { stop, handle }
    }
}

impl Drop for DeviceMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn poll_loop(signals: &Sender<DeviceSignal>, stop: &AtomicBool) {
    let mut last = device::enumerate_output_devices();
    loop {
        // 分片 sleep，析构时最多等一片
        for _ in 0..SLICES {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(POLL_SLICE);
        }

        let current = device::enumerate_output_devices();
        if current == last {
            continue;
        }
        last = current;

        // 控制线程退出后接收端关闭，send 失败即收工
        if signals.send(DeviceSignal::TopologyChanged).is_err() {
            return;
        }
    }
}
