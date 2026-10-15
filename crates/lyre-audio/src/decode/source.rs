//! 无锁解码源，解码跑在专属线程 `mp-decode`，经有界 crossbeam 通道送样本。
//! rodio 的 mixer 是在 cpal 音频回调线程上拉 source 的，解码和文件 IO 绝不能进回调。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError, bounded};
use rodio::Source;
use rodio::{ChannelCount, SampleRate};

use crate::error::{AudioError, Result};

use super::track::TrackDecoder;

/// 畸形文件头（声道数写 0）时尝试使用立体声。
const DEFAULT_CHANNELS: ChannelCount = match std::num::NonZeroU16::new(2) {
    Some(value) => value,
    None => ChannelCount::MIN,
};

/// 畸形文件头（采样率写 0）时直接尝试 48kHz。
const DEFAULT_SAMPLE_RATE: SampleRate = match std::num::NonZeroU32::new(48_000) {
    Some(value) => value,
    None => SampleRate::MIN,
};

const CHUNK: usize = 16_384;

/// 有界通道容量（块数）：约 5 秒 48kHz 立体声 PCM（约 2MB）。
const CHANNEL_CAPACITY_CHUNKS: usize = 30;

/// 解码线程检查停止标志的间隔（样本数）。`drop` 可能发生在音频回调线程上，
/// 把检查铺进采样循环能把退出延迟压在一个数据包之内。
const STOP_CHECK_INTERVAL: usize = 256;

/// [`WhenEmpty::Wait`] 的单次等待上限。它同时是换手时收工延迟的上限：
/// 推送线程卡在等里头时收不到取消标志，只能等这一觉醒来；解码卡死也靠兜底静音放行，不能无限等。
const WAIT_LIMIT: Duration = Duration::from_millis(200);

/// 通道空着、解码还没追上时怎么办 —— 空时吐出的 `0.0` 会被下游当成曲子的内容。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WhenEmpty {
    /// 立刻吐静音顶上。rodio 的 mixer 在音频回调线程上拉，那里绝不能阻塞。
    /// 亏欠只有一次回调那么多（几十毫秒以内），代价可以接受。
    PadWithSilence,
    /// 先等一等真样本（最长 `WAIT_LIMIT`），等不到才兜底吐静音。
    /// 给独占输出的推送线程用：它不在实时线程上，快过解码线程是常态
    /// （实测 FLAC 开头会被灌进约 480ms、WavPack 约 2.5s 的补白），补白进了环就成了曲子里的沉默段。
    Wait,
}

pub struct LockFreeSource {
    receiver: Option<Receiver<Vec<f32>>>,
    stop_flag: Arc<AtomicBool>,
    producer_finished: Arc<AtomicBool>,
    channels: u16,
    sample_rate: u32,
    total_duration: Option<Duration>,
    /// 当前正在消费的那一块，发送方只投递非空块。
    chunk: Vec<f32>,
    chunk_pos: usize,
    /// 是否处于欠载态。
    stalled: bool,
    when_empty: WhenEmpty,
}

impl LockFreeSource {
    pub fn new(mut decoder: TrackDecoder, when_empty: WhenEmpty) -> Result<Self> {
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let total_duration = decoder.total_duration();

        // 起线程之前先解一段出来，避免音频回调一上来就饿着。
        // 解不开必须报错：吞掉它这首会一上来就「放完」，列表循环时像队列在无声快进。
        decoder.prefill()?;

        let (sender, receiver) = bounded(CHANNEL_CAPACITY_CHUNKS);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let producer_finished = Arc::new(AtomicBool::new(false));

        let stop = Arc::clone(&stop_flag);
        let finished = Arc::clone(&producer_finished);

        thread::Builder::new()
            .name("mp-decode".to_owned())
            .spawn(move || {
                let mut batch = Vec::with_capacity(CHUNK);
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    for i in 0..CHUNK {
                        if i % STOP_CHECK_INTERVAL == 0 && stop.load(Ordering::Relaxed) {
                            break;
                        }
                        match decoder.next() {
                            Some(sample) => batch.push(sample),
                            None => break,
                        }
                    }
                    if batch.is_empty() {
                        break;
                    }
                    // 通道满了就在这里等，天然形成背压
                    let payload = std::mem::take(&mut batch);
                    if sender.send(payload).is_err() {
                        finished.store(true, Ordering::Release);
                        return;
                    }
                    // 所有权随 `send` 走了，下一块在这里重新长出来
                    batch = Vec::with_capacity(CHUNK);
                }
                finished.store(true, Ordering::Release);
            })
            .map_err(AudioError::ThreadSpawn)?;

        Ok(Self {
            receiver: Some(receiver),
            stop_flag,
            producer_finished,
            channels,
            sample_rate,
            total_duration,
            chunk: Vec::new(),
            chunk_pos: 0,
            stalled: false,
            when_empty,
        })
    }

    /// 解码线程是否已经收工。
    pub fn producer_finished(&self) -> bool {
        self.producer_finished.load(Ordering::Acquire)
    }

    /// 回调线程当前是否在靠静音撑流，说明解码跟不上播放。
    pub fn is_stalled(&self) -> bool {
        self.stalled
    }
}

impl Iterator for LockFreeSource {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        if self.chunk_pos < self.chunk.len() {
            let sample = self.chunk[self.chunk_pos];
            self.chunk_pos += 1;
            return Some(sample);
        }

        match self.fetch() {
            Fetched::Batch(batch) => {
                self.stalled = false;
                self.chunk = batch;
                self.chunk_pos = 0;
            }
            Fetched::Done => return None,
            Fetched::Starved => {
                // 欠载时吐静音撑住输出流，绝不在这里阻塞（`Wait` 已经先等过一轮了）
                self.stalled = true;
                return Some(0.0);
            }
        }

        // 发送方只在攒到样本时才投递，所以块不会为空；真出现也只是提前结束，不该 panic
        let first = self.chunk.first().copied()?;
        self.chunk_pos = 1;
        Some(first)
    }
}

/// 取下一块的结果。
enum Fetched {
    Batch(Vec<f32>),
    /// 解码线程收工且通道已空 —— 这首放完了。
    Done,
    /// 通道空着、解码还没追上。
    Starved,
}

impl LockFreeSource {
    /// 取下一块。通道空着时按 [`WhenEmpty`] 决定是等还是直接认输。
    fn fetch(&self) -> Fetched {
        let Some(receiver) = self.receiver.as_ref() else {
            return Fetched::Done;
        };

        match receiver.try_recv() {
            Ok(batch) => Fetched::Batch(batch),
            Err(TryRecvError::Disconnected) => Fetched::Done,
            Err(TryRecvError::Empty) => {
                // 「解码线程收工」且「通道确实空了」才算放完；单看 empty 会把欠载误判成结束
                if self.finished(receiver) {
                    return Fetched::Done;
                }
                if self.when_empty == WhenEmpty::Wait {
                    match receiver.recv_timeout(WAIT_LIMIT) {
                        Ok(batch) => return Fetched::Batch(batch),
                        Err(RecvTimeoutError::Disconnected) => return Fetched::Done,
                        Err(RecvTimeoutError::Timeout) => {
                            if self.finished(receiver) {
                                return Fetched::Done;
                            }
                        }
                    }
                }
                Fetched::Starved
            }
        }
    }

    fn finished(&self, receiver: &Receiver<Vec<f32>>) -> bool {
        self.producer_finished.load(Ordering::Acquire) && receiver.is_empty()
    }
}

impl Source for LockFreeSource {
    /// 恒返回 `None`：本源跨度未知，rodio 因此不会按跨度做重采样 bootstrap ——
    /// 必须在交给 rodio 之前显式把采样率对齐到 mixer（见 `rodio_backend.rs`）。
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        std::num::NonZeroU16::new(self.channels).unwrap_or(DEFAULT_CHANNELS)
    }

    fn sample_rate(&self) -> SampleRate {
        std::num::NonZeroU32::new(self.sample_rate).unwrap_or(DEFAULT_SAMPLE_RATE)
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }
}

impl Drop for LockFreeSource {
    fn drop(&mut self) {
        // 只置 stop 标志、不 join：解码线程靠每 256 个样本的检查自行退出。
        self.stop_flag.store(true, Ordering::Relaxed);
        drop(self.receiver.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{Receiver, bounded};

    /// 只带通道的空壳，不必真的起解码线程。
    fn bare(
        receiver: Receiver<Vec<f32>>,
        when_empty: WhenEmpty,
        producer_finished: bool,
    ) -> LockFreeSource {
        LockFreeSource {
            receiver: Some(receiver),
            stop_flag: Arc::new(AtomicBool::new(false)),
            producer_finished: Arc::new(AtomicBool::new(producer_finished)),
            channels: 2,
            sample_rate: 48_000,
            total_duration: None,
            chunk: Vec::new(),
            chunk_pos: 0,
            stalled: false,
            when_empty,
        }
    }

    /// 实时回调那条路只能吐静音顶上：回调里不能阻塞。
    #[test]
    fn padding_mode_hands_out_silence_instead_of_waiting() {
        let (_sender, receiver) = bounded(4);
        let mut source = bare(receiver, WhenEmpty::PadWithSilence, false);

        assert_eq!(source.next(), Some(0.0));
        assert!(source.is_stalled(), "吐了静音就该报欠载");
    }

    /// 独占推送线程那条路要**等**真样本：它吐出来的静音会变成曲子里的沉默段。
    #[test]
    fn waiting_mode_waits_for_real_samples_instead_of_inventing_silence() {
        let (sender, receiver) = bounded(4);
        let mut source = bare(receiver, WhenEmpty::Wait, false);

        let puller = thread::spawn(move || source.next());
        thread::sleep(Duration::from_millis(60));
        assert!(
            !puller.is_finished(),
            "通道空着时应当等着，而不是立刻给出一个补出来的 0.0"
        );

        sender.send(vec![0.5, 0.25]).expect("送一批真样本");
        assert_eq!(
            puller.join().expect("取回结果"),
            Some(0.5),
            "等到的应当是真样本本身"
        );
    }

    #[test]
    fn waiting_mode_still_delivers_the_whole_batch() {
        let (sender, receiver) = bounded(4);
        sender.send(vec![1.0, 2.0, 3.0]).expect("先备一批");
        let mut source = bare(receiver, WhenEmpty::Wait, false);

        let got: Vec<f32> = source.by_ref().take(3).collect();
        assert_eq!(got, vec![1.0, 2.0, 3.0]);
    }

    /// 解码线程收工且通道已空 = 这首放完了，`Wait` 不能在这儿一直等下去。
    #[test]
    fn waiting_mode_ends_instead_of_waiting_forever() {
        let (_sender, receiver) = bounded(4);
        let mut source = bare(receiver, WhenEmpty::Wait, true);
        assert_eq!(source.next(), None);
    }

    /// 发送端全没了也当放完，不该卡在等里头。
    #[test]
    fn waiting_mode_ends_when_the_sender_is_gone() {
        let (sender, receiver) = bounded::<Vec<f32>>(4);
        drop(sender);
        let mut source = bare(receiver, WhenEmpty::Wait, false);
        assert_eq!(source.next(), None);
    }
}
