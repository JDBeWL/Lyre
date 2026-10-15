//! 独占输出的采样环：解码推送线程写，渲染线程读，基于 `rtrb` 的 SPSC 环。
//! 必须有写世代：切歌或 seek 时旧推送线程可能卡在水位门上，醒来后会灌进上一段的样本，靠世代作废。
//! 生产者那半放在互斥量里：它要在控制线程与推送线程之间交接（每次切歌换一个），实时渲染线程不碰它。

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rtrb::{Consumer, Producer, RingBuffer};

/// 环按「最坏情况约 2 秒」一次性分配：192kHz × 8 声道 × 2s。
/// 容量必须在协商格式之前定下来（环比设备先建好），所以按上限给；实际水位由推送线程的门控决定。
pub const RING_SAMPLES: usize = 192_000 * 8 * 2;

/// 丢弃旧世代的残留时的临时缓冲长度。
const DISCARD_CHUNK: usize = 4096;

pub struct SampleRing {
    producer: Mutex<Producer<f32>>,
    /// 世代号，[`SampleRing::invalidate`] 每调用一次加一。
    epoch: AtomicU64,
    /// 最近一次作废时环里的存量样本数，它们全部属于被作废的那一代。
    ///
    /// 发布顺序不能反：写入端先写 `stale` 再发布 `epoch`（release），读取端先读 `epoch`
    /// （acquire）再读 `stale`，否则新世代号配上旧存量会把刚写进来的样本一起丢掉。
    stale: AtomicUsize,
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
            stale: AtomicUsize::new(0),
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
            epoch: ring.epoch(),
            ring: Arc::clone(ring),
        }
    }

    /// 作废当前世代：**作废那一刻**环里的样本会被读取端丢掉，还持着旧世代的写入端从此写不进东西。
    /// 作废之后写进来的属于新世代，照常交付。
    pub fn invalidate(&self) {
        // 存量数与世代号都在锁里动：写入端也在这把锁下核对世代，于是「量完之后才写进来」
        // 的样本不可能被算进旧的存量里。
        let producer = self.lock();
        self.stale.store(
            producer.buffer().capacity() - producer.slots(),
            Ordering::Relaxed,
        );
        self.epoch.fetch_add(1, Ordering::Release);
    }

    /// 已经写进去但还没被读走的样本数。
    pub fn backlog(&self) -> usize {
        let producer = self.lock();
        producer.buffer().capacity() - producer.slots()
    }

    fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    fn stale(&self) -> usize {
        self.stale.load(Ordering::Relaxed)
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

    /// 写一批采样，返回真正写进去的个数。返回 0 有两种含义，调用方都应当收工：
    /// 世代已过期（本线程的样本作废），或环已满（水位门控失效，属异常路径）。
    pub fn write(&self, samples: &[f32]) -> usize {
        let mut producer = self.ring.lock();
        // 世代要在拿到锁之后再核对一次：先在锁外看一眼的话，「作废」有可能挤在两者之间
        // 完成，于是上一代的样本被当成新世代的数据播出去。
        if self.epoch != self.ring.epoch() {
            return 0;
        }

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
    /// 环里还有多少样本可读。
    pub fn pending(&self) -> usize {
        self.consumer.slots()
    }

    /// 读一批采样，返回读到的个数；不足的部分由调用方补静音。
    pub fn read(&mut self, out: &mut [f32]) -> usize {
        let epoch = self.ring.epoch();
        if epoch != self.epoch {
            self.epoch = epoch;
            // 只丢作废那一刻的存量。作废之后写进来的是新一代的数据，留着当场就能读出去 ——
            // 整环清空的话，切歌时新曲目开头那一段就被白扔了。
            self.discard(self.ring.stale());
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

    /// 丢掉 `count` 个样本（不足就只丢环里现有的）。
    fn discard(&mut self, mut count: usize) {
        let mut scratch = [0.0f32; DISCARD_CHUNK];
        while count > 0 {
            let take = self.consumer.slots().min(DISCARD_CHUNK).min(count);
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
            count -= take;
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

    #[test]
    fn a_full_ring_reports_zero_written() {
        let (_ring, writer, _reader) = SampleRing::new();
        assert_eq!(writer.write(&vec![0.5f32; RING_SAMPLES]), RING_SAMPLES);
        assert_eq!(writer.write(&[0.1, 0.2]), 0, "满了就该写不进去");
    }

    /// 作废之后写进来的样本属于新世代：读取端第一次读要丢掉旧存量、同时当场交出新的。
    #[test]
    fn a_swap_discards_only_what_was_there_before_it() {
        let (ring, writer, mut reader) = SampleRing::new();
        assert_eq!(writer.write(&[0.9f32; 512]), 512);

        ring.invalidate();

        // 新推送线程在渲染线程还没反应过来之前就灌进了一整段
        let fresh = SampleRing::writer(&ring);
        assert_eq!(fresh.write(&[0.25f32; 1024]), 1024);

        let mut out = vec![0.0f32; 1024];
        assert_eq!(
            reader.read(&mut out),
            1024,
            "新世代的样本必须在第一次读时就交出来"
        );
        assert!(
            out.iter().all(|sample| *sample == 0.25),
            "交出来的应当是新一代的样本，旧世代的残留一个都不该混进来"
        );
        assert_eq!(ring.backlog(), 0);
    }

    /// 连续两次作废（切歌还没消费完又 seek）之后，环里剩下的全是可丢的：上一次作废之后
    /// 写的也一并作废。存量要按最新一次量的算，不能两次叠起来多丢。
    #[test]
    fn back_to_back_swaps_do_not_stack_up() {
        let (ring, writer, mut reader) = SampleRing::new();
        writer.write(&[0.5f32; 300]);
        ring.invalidate();

        let first = SampleRing::writer(&ring);
        first.write(&[0.6f32; 700]);
        ring.invalidate();

        let second = SampleRing::writer(&ring);
        second.write(&[0.7f32; 400]);

        // 读取端从头到尾没读过：旧存量 300 + 第一次作废后写的 700 都是可丢的，只剩 400
        let mut out = vec![0.0f32; 400];
        assert_eq!(reader.read(&mut out), 400);
        assert!(out.iter().all(|sample| *sample == 0.7));
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
