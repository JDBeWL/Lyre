//! 解码推送线程：把播放链路的输出灌进采样环。
//!
//! 这不是实时线程，可以用带节奏的取数：水位高就等着渲染线程消费，而不是把静音直接灌进环充数。
//!
//! 只有「源真的读完了」才置 `done`。取消（切歌、停流、换设备）不算播完 ——
//! 否则被取消的旧线程会在新的一首刚开始时把「整首放完」报上去，界面当场跳下一首。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::error::{ExclusiveError, Result};
use crate::ring::{RING_SAMPLES, RingWriter};

/// 水位上限（秒）。超过就等渲染线程消费，不把内存撑爆。
const WATERMARK_SECONDS: usize = 2;
/// 水位门控等待的单次上限。取消能被及时看到，全靠在它上面切时间。
const GATE_TICK: Duration = Duration::from_millis(20);
/// 一次搬运的样本数，约 170ms @48k 立体声。
const CHUNK_SAMPLES: usize = 16_384;

/// 推送线程的句柄。
pub struct PushThread {
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PushThread {
    /// 让推送线程收工并等它退出。等待有界：它最多睡一个 [`GATE_TICK`]。
    pub fn join(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PushThread {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 起一条推送线程。`done` 由调用方持有并转交渲染线程。
pub fn spawn<S>(
    mut source: S,
    writer: RingWriter,
    sample_rate: u32,
    channels: u16,
    done: Arc<AtomicBool>,
) -> Result<PushThread>
where
    S: Iterator<Item = f32> + Send + 'static,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = Arc::clone(&cancel);

    let handle = thread::Builder::new()
        .name("lyre-exclusive-push".to_owned())
        .spawn(move || {
            let per_second = sample_rate as usize * channels as usize;
            // 水位不能顶到环的上限，否则门控刚好卡在「一点空间都不剩」上，写块永远塞不进去
            let watermark = (per_second * WATERMARK_SECONDS)
                .min(RING_SAMPLES.saturating_sub(CHUNK_SAMPLES * 2));

            let mut buffer: Vec<f32> = Vec::with_capacity(CHUNK_SAMPLES);
            let mut exhausted = false;

            loop {
                if thread_cancel.load(Ordering::Relaxed) {
                    break;
                }

                while writer.backlog() >= watermark {
                    if thread_cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(GATE_TICK);
                }

                buffer.clear();
                buffer.extend(source.by_ref().take(CHUNK_SAMPLES));
                if buffer.is_empty() {
                    exhausted = true;
                    break;
                }
                let short = buffer.len() < CHUNK_SAMPLES;

                // 返回 0 只有两种含义：世代过期（本线程的样本已作废）或环满，都该收工
                let written = writer.write(&buffer);
                if written < buffer.len() {
                    if written == 0 {
                        log::debug!("独占推送：世代已过期或环已满，停止本线程");
                    } else {
                        log::warn!(
                            "独占推送：只写进 {written}/{} 个采样，水位门控失效",
                            buffer.len()
                        );
                    }
                    return;
                }

                if short {
                    exhausted = true;
                    break;
                }
            }

            if exhausted {
                done.store(true, Ordering::Release);
            }
        })
        .map_err(ExclusiveError::ThreadSpawn)?;

    Ok(PushThread {
        cancel,
        handle: Some(handle),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::SampleRing;

    /// 每个样本都要恰好到达一次，一个不丢、一个不重。
    #[test]
    fn every_sample_reaches_the_ring_exactly_once() {
        const TOTAL: usize = 200_000;

        let (ring, writer, mut reader) = SampleRing::new();
        let done = Arc::new(AtomicBool::new(false));
        let push = spawn(
            (0..TOTAL).map(|i| i as f32),
            writer,
            48_000,
            2,
            Arc::clone(&done),
        )
        .expect("推送线程应当起得来");

        let mut collected: Vec<f32> = Vec::with_capacity(TOTAL);
        let mut buffer = vec![0.0f32; 4096];
        let mut idle = 0;
        while collected.len() < TOTAL {
            let got = reader.read(&mut buffer);
            if got == 0 {
                // 推送结束且环已排空才算真的没了
                if done.load(Ordering::Acquire) && ring.backlog() == 0 {
                    break;
                }
                idle += 1;
                assert!(idle < 2_000, "推送迟迟不结束，可能卡在水位门控上了");
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            collected.extend_from_slice(&buffer[..got]);
        }
        push.join();

        assert_eq!(collected.len(), TOTAL, "样本数不对，说明有丢或有多");
        assert!(
            collected
                .iter()
                .enumerate()
                .all(|(index, sample)| *sample == index as f32),
            "样本顺序被打乱了"
        );
    }

    /// 取消不等于播完：被取消的线程绝不能把 `done` 置起来。
    #[test]
    fn cancelling_stops_the_thread_without_claiming_completion() {
        let (_ring, writer, _reader) = SampleRing::new();
        let done = Arc::new(AtomicBool::new(false));

        // 无穷源：水位一满就会停在门控上，只有取消能叫醒它
        let push = spawn(
            std::iter::repeat_n(0.0f32, usize::MAX),
            writer,
            48_000,
            2,
            Arc::clone(&done),
        )
        .expect("推送线程应当起得来");

        thread::sleep(Duration::from_millis(30));
        push.join();

        assert!(!done.load(Ordering::Acquire), "取消不是「播完了」");
    }

    /// 源比一块还短：仍然要写完，并且如实报「读完了」。
    #[test]
    fn a_source_shorter_than_one_chunk_still_completes() {
        let (_ring, writer, mut reader) = SampleRing::new();
        let done = Arc::new(AtomicBool::new(false));
        let push = spawn(
            vec![0.5f32; 100].into_iter(),
            writer,
            48_000,
            2,
            Arc::clone(&done),
        )
        .expect("推送线程应当起得来");

        // 先等它自己跑完再 join：join 会先置取消标志，抢在它开跑之前就把完成标志掐掉了
        for _ in 0..200 {
            if done.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        push.join();

        assert!(done.load(Ordering::Acquire), "读完了就该报完成");
        let mut out = vec![0.0f32; 100];
        assert_eq!(reader.read(&mut out), 100);
        assert!(out.iter().all(|sample| *sample == 0.5));
    }
}
