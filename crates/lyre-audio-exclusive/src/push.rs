//! 解码推送线程：把播放链路的输出灌进采样环。不是实时线程，可以按水位有节奏地取数。
//! 只有「源真的读完了」才置 `done`：取消（切歌、停流、换设备）不算播完 ——
//! 被取消的旧线程若报「整首放完」，界面会在新的一首刚开始时当场跳下一首。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// 起一条推送线程。`done` 由调用方持有并转交渲染线程，写进去的是这一代的世代号。
pub fn spawn<S>(
    mut source: S,
    writer: RingWriter,
    sample_rate: u32,
    channels: u16,
    done: Arc<AtomicU64>,
    generation: u64,
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
                done.store(generation, Ordering::Release);
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

    /// 用例里统一用第 1 代，方便断言完工标志写进去的是不是这个号。
    const GENERATION: u64 = 1;

    #[test]
    fn every_sample_reaches_the_ring_exactly_once() {
        const TOTAL: usize = 200_000;

        let (ring, writer, mut reader) = SampleRing::new();
        let done = Arc::new(AtomicU64::new(0));
        let push = spawn(
            (0..TOTAL).map(|i| i as f32),
            writer,
            48_000,
            2,
            Arc::clone(&done),
            GENERATION,
        )
        .expect("推送线程应当起得来");

        let mut collected: Vec<f32> = Vec::with_capacity(TOTAL);
        let mut buffer = vec![0.0f32; 4096];
        let mut idle = 0;
        while collected.len() < TOTAL {
            let got = reader.read(&mut buffer);
            if got == 0 {
                // 推送结束且环已排空才算真的没了
                if done.load(Ordering::Acquire) == GENERATION && ring.backlog() == 0 {
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

    #[test]
    fn cancelling_stops_the_thread_without_claiming_completion() {
        let (_ring, writer, _reader) = SampleRing::new();
        let done = Arc::new(AtomicU64::new(0));

        // 无穷源：水位一满就会停在门控上，只有取消能叫醒它
        let push = spawn(
            std::iter::repeat_n(0.0f32, usize::MAX),
            writer,
            48_000,
            2,
            Arc::clone(&done),
            GENERATION,
        )
        .expect("推送线程应当起得来");

        thread::sleep(Duration::from_millis(30));
        push.join();

        assert_ne!(
            done.load(Ordering::Acquire),
            GENERATION,
            "取消不是「播完了」"
        );
    }

    #[test]
    fn a_source_shorter_than_one_chunk_still_completes() {
        let (_ring, writer, mut reader) = SampleRing::new();
        let done = Arc::new(AtomicU64::new(0));
        let push = spawn(
            vec![0.5f32; 100].into_iter(),
            writer,
            48_000,
            2,
            Arc::clone(&done),
            GENERATION,
        )
        .expect("推送线程应当起得来");

        // 先等它自己跑完再 join：join 会先置取消标志，抢在它开跑之前就把完成标志掐掉了
        for _ in 0..200 {
            if done.load(Ordering::Acquire) == GENERATION {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        push.join();

        assert_eq!(done.load(Ordering::Acquire), GENERATION, "读完了就该报完成");
        let mut out = vec![0.0f32; 100];
        assert_eq!(reader.read(&mut out), 100);
        assert!(out.iter().all(|sample| *sample == 0.5));
    }

    /// 复刻切歌的真实时序：旧推送线程把环灌满 → 停掉它 → 作废世代 → 起新推送线程 →
    /// 渲染线程这才第一次读。新曲目的样本必须一个不少地交出来。
    #[test]
    fn a_track_swap_hands_the_new_buffer_over_intact() {
        const OLD: usize = 100_000;
        const NEW: usize = 40_000;

        let (ring, writer, mut reader) = SampleRing::new();
        let done_old = Arc::new(AtomicU64::new(0));
        let old = spawn(
            std::iter::repeat_n(0.9f32, OLD),
            writer,
            48_000,
            2,
            Arc::clone(&done_old),
            GENERATION,
        )
        .expect("推送线程应当起得来");

        // 素材比水位短，它一口气写完就收工，不必等门控
        for _ in 0..200 {
            if done_old.load(Ordering::Acquire) == GENERATION {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(ring.backlog(), OLD, "旧世代应当已经把环灌满");

        old.join();
        ring.invalidate();
        let done_new = Arc::new(AtomicU64::new(0));
        let fresh = spawn(
            (0..NEW).map(|index| index as f32),
            SampleRing::writer(&ring),
            48_000,
            2,
            Arc::clone(&done_new),
            GENERATION + 1,
        )
        .expect("推送线程应当起得来");

        // 关键：新推送线程先把环灌起来，渲染线程**才**醒过来读。
        // 换手时渲染线程不是在等设备事件、就是在空转打盹，先醒来的是推送线程 —— 这就是丢音的那条路。
        for _ in 0..400 {
            if done_new.load(Ordering::Acquire) == GENERATION + 1 {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            done_new.load(Ordering::Acquire),
            GENERATION + 1,
            "新推送线程应当已经写完"
        );
        assert_eq!(
            ring.backlog(),
            OLD + NEW,
            "环里应当同时躺着旧世代的残留与新一代的全部样本"
        );

        let mut out = vec![0.0f32; NEW];
        let mut got = 0;
        for _ in 0..400 {
            got += reader.read(&mut out[got..]);
            if got == NEW {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        fresh.join();

        assert_eq!(got, NEW, "新曲目的开头顶掉了 {got}/{NEW} 个样本");
        assert!(
            out.iter()
                .enumerate()
                .all(|(index, sample)| *sample == index as f32),
            "交出来的样本顺序不对，或掺进了旧世代的残留"
        );
    }
}
