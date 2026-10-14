//! 独占输出的采样环：解码推送线程写，渲染线程读。
//!
//! 用 `rtrb` 提供的安全 SPSC 环，不手写 `UnsafeCell`。写世代（epoch）是必须的：
//! 切歌或 seek 时旧推送线程可能正卡在水位门上，醒来后会把上一段的样本灌进来 ——
//! 靠世代作废比靠「记得在每个循环里查标志」可靠。
//!
//! 生产者那半放在互斥量里，是因为它要在控制线程与推送线程之间交接（每次切歌换一个推送
//! 线程）。争用可以忽略：推送线程一个数据块才锁一次，而实时渲染线程根本不碰它。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::{Consumer, Producer, RingBuffer};

/// 环按「最坏情况约 2 秒」一次性分配：192kHz × 8 声道 × 2s。
///
/// 容量必须在协商格式之前定下来（环比设备先建好），所以按上限给，
/// 实际协商到 44.1k 立体声时只是富余而已。真正的水位由推送线程的门控决定。
pub const RING_SAMPLES: usize = 192_000 * 8 * 2;

/// 丢弃旧世代的残留时的临时缓冲长度。
const DISCARD_CHUNK: usize = 4096;

/// 环的共享状态：生产者、写世代。
pub struct SampleRing {
    producer: Mutex<Producer<f32>>,
    epoch: AtomicU64,
}

/// 推送线程持有的写入端。
pub struct RingWriter {
    epoch: u64,
    ring: Arc<SampleRing>,
}

/// 渲染线程持有的读取端。
pub struct RingReader {
    consumer: Consumer<f32>,
    epoch: u64,
    ring: Arc<SampleRing>,
}

impl SampleRing {
    /// 建环，返回共享状态与两个端；两个端各自只应由一条线程使用。
    pub fn new() -> (Arc<Self>, RingWriter, RingReader) {
        let (producer, consumer) = RingBuffer::new(RING_SAMPLES);
        let ring = Arc::new(Self {
            producer: Mutex::new(producer),
            epoch: AtomicU64::new(0),
        });

        let writer = Self::writer(&ring);
        let reader = RingReader {
            consumer,
            epoch: 0,
            ring: Arc::clone(&ring),
        };
        (ring, writer, reader)
    }

    /// 给一条新的推送线程取写入端。同一时刻只应存在一个。
    pub fn writer(ring: &Arc<Self>) -> RingWriter {
        RingWriter {
            epoch: ring.epoch.load(Ordering::Acquire),
            ring: Arc::clone(ring),
        }
    }

    /// 作废当前世代：环里残留下来的样本会被读取端丢掉，还持着旧世代的写入端从此写不进东西。
    pub fn invalidate(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// 已经写进去但还没被读走的样本数。
    pub fn backlog(&self) -> usize {
        let producer = self.lock();
        producer.buffer().capacity() - producer.slots()
    }

    /// 锁中毒只说明某个持有者 panic 过，环本身没坏，照常取用。
    fn lock(&self) -> MutexGuard<'_, Producer<f32>> {
        self.producer.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl RingWriter {
    /// 已经写进去但还没被读走的样本数，水位门控用它。
    pub fn backlog(&self) -> usize {
        self.ring.backlog()
    }

    /// 写一批采样，返回真正写进去的个数。
    ///
    /// 返回 0 有两种含义，调用方都应当收工：世代已过期（本线程的样本作废），
    /// 或环已满（水位门控失效，属异常路径）。
    pub fn write(&self, samples: &[f32]) -> usize {
        if self.epoch != self.ring.epoch.load(Ordering::Acquire) {
            return 0;
        }

        let mut producer = self.ring.lock();
        let take = producer.slots().min(samples.len());
        if take == 0 {
            return 0;
        }
        if producer.push_entire_slice(&samples[..take]).is_err() {
            return 0;
        }
        take
    }
}

impl RingReader {
    /// 环里还有多少样本可读。只查不动。
    pub fn pending(&self) -> usize {
        self.consumer.slots()
    }

    /// 读一批采样，返回读到的个数；不足的部分由调用方补静音。
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        if self.epoch != self.ring.epoch.load(Ordering::Acquire) {
            // 世代变了：环里剩的全是上一段的东西，全部丢掉
            self.epoch = self.ring.epoch.load(Ordering::Acquire);
            self.discard_all();
        }

        let take = self.consumer.slots().min(out.len());
        if take == 0 {
            return 0;
        }
        if self.consumer.pop_entire_slice(&mut out[..take]).is_err() {
            return 0;
        }
        take
    }

    fn discard_all(&mut self) {
        let mut scratch = [0.0f32; DISCARD_CHUNK];
        loop {
            let take = self.consumer.slots().min(DISCARD_CHUNK);
            if take == 0 {
                return;
            }
            if self
                .consumer
                .pop_entire_slice(&mut scratch[..take])
                .is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_come_out_in_order() {
        let (_ring, writer, mut reader) = SampleRing::new();

        let sent: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        assert_eq!(writer.write(&sent), 1000);

        let mut out = vec![0.0f32; 1000];
        assert_eq!(reader.read(&mut out), 1000);
        assert_eq!(out, sent);
    }

    /// 单次写入超过环容量时只写进去环能装下的部分，不能丢数据也不能 panic。
    #[test]
    fn a_write_larger_than_the_ring_is_clamped_not_dropped() {
        let (ring, writer, mut reader) = SampleRing::new();
        let huge = vec![1.0f32; RING_SAMPLES + 500];

        assert_eq!(writer.write(&huge), RING_SAMPLES);
        assert_eq!(ring.backlog(), RING_SAMPLES);

        let mut out = vec![0.0f32; RING_SAMPLES];
        assert_eq!(reader.read(&mut out), RING_SAMPLES);
        assert!(out.iter().all(|sample| *sample == 1.0));
        assert_eq!(ring.backlog(), 0);
    }

    /// 环满之后再写应当如实返回 0，调用方据此停手。
    #[test]
    fn a_full_ring_reports_zero_written() {
        let (_ring, writer, _reader) = SampleRing::new();
        assert_eq!(writer.write(&vec![0.5f32; RING_SAMPLES]), RING_SAMPLES);
        assert_eq!(writer.write(&[0.1, 0.2]), 0, "满了就该写不进去");
    }

    /// 作废之后旧写入端一律写不进，环里残留的样本也要被读取端丢弃 ——
    /// 否则切歌后的头几毫秒会掺进上一段的尾巴。
    #[test]
    fn an_invalidated_generation_can_neither_write_nor_read_stale_samples() {
        let (ring, writer, mut reader) = SampleRing::new();
        assert_eq!(writer.write(&[0.9f32; 512]), 512);

        ring.invalidate();

        assert_eq!(writer.write(&[0.1]), 0, "旧世代不该还能写进去");

        let mut out = vec![0.0f32; 512];
        assert_eq!(reader.read(&mut out), 0);
        assert!(
            out.iter().all(|sample| *sample == 0.0),
            "旧世代的样本不该被交出去"
        );
    }

    /// 作废只针对旧写入端：新世代照常收发。
    #[test]
    fn a_new_generation_works_again_after_invalidation() {
        let (ring, writer, mut reader) = SampleRing::new();
        writer.write(&[1.0; 8]);
        ring.invalidate();

        let mut out = vec![0.0f32; 8];
        reader.read(&mut out); // 丢掉旧的

        let fresh = SampleRing::writer(&ring);
        assert_eq!(fresh.write(&[0.25; 8]), 8);
        assert_eq!(reader.read(&mut out), 8);
        assert!(out.iter().all(|sample| *sample == 0.25));
    }

    #[test]
    fn backlog_tracks_what_is_still_unread() {
        let (ring, writer, mut reader) = SampleRing::new();
        writer.write(&[0.0; 256]);
        assert_eq!(writer.backlog(), 256);
        assert_eq!(ring.backlog(), 256);

        let mut out = vec![0.0f32; 100];
        reader.read(&mut out);
        assert_eq!(writer.backlog(), 156);
    }
}
