//! 无锁解码源，解码跑在专属线程 `mp-decode`，经有界 crossbeam 通道送样本。
//! rodio 的 mixer 是在 cpal 音频回调线程上拉 source 的，解码和文件 IO 绝不能进回调。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, TryRecvError, bounded};
use rodio::Source;
use rodio::{ChannelCount, SampleRate};

use crate::error::{AudioError, Result};

use super::decoder::SymphoniaDecoder;

/// 畸形文件头（声道数写 0）时尝试使用立体声。
/// 用 `match` 而不是 `NonZeroU16::new(2).unwrap()` 是因为 pedantic 会拦下常量上的 unwrap。
const DEFAULT_CHANNELS: ChannelCount = match std::num::NonZeroU16::new(2) {
    Some(value) => value,
    None => ChannelCount::MIN,
};

/// 畸形文件头（采样率写 0）时直接尝试48kHz。
const DEFAULT_SAMPLE_RATE: SampleRate = match std::num::NonZeroU32::new(48_000) {
    Some(value) => value,
    None => SampleRate::MIN,
};

/// 通道里传的是整块采样而不是单个样本，一次入队的开销摊到整块上。
const CHUNK: usize = 16_384;

/// 有界通道容量（块数）：约 5 秒 48kHz 立体声 PCM（约 2MB）。
const CHANNEL_CAPACITY_CHUNKS: usize = 30;

/// 解码线程检查停止标志的间隔（样本数）。`drop` 可能发生在音频回调线程上并 join，
/// 把检查铺进采样循环能把退出延迟压在一个数据包之内。
const STOP_CHECK_INTERVAL: usize = 256;

pub struct LockFreeSource {
    receiver: Option<Receiver<Vec<f32>>>,
    stop_flag: Arc<AtomicBool>,
    producer_finished: Arc<AtomicBool>,
    channels: u16,
    sample_rate: u32,
    total_duration: Option<Duration>,
    /// 当前正在消费的那一块。发送方只在非空时才投递，所以它一旦有内容就一定有样本。
    chunk: Vec<f32>,
    chunk_pos: usize,
    /// 是否处于欠载态。
    stalled: bool,
}

impl LockFreeSource {
    pub fn new(mut decoder: SymphoniaDecoder) -> Result<Self> {
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let total_duration = decoder.total_duration();

        // 起线程之前先解一段出来，避免音频回调一上来就饿着。
        // 填不上就是「这个文件解不开」，吞掉它的后果是这首一上来就「放完」，
        // 列表循环时看起来像队列在无声快进。
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

        let receiver = self.receiver.as_ref()?;

        match receiver.try_recv() {
            Ok(batch) => {
                self.stalled = false;
                // 整块接管所有权，不逐样本再搬一遍
                self.chunk = batch;
                self.chunk_pos = 0;
            }
            Err(TryRecvError::Disconnected) => return None,
            Err(TryRecvError::Empty) => {
                // 「解码线程收工」且「通道确实空了」才算放完；单看 empty 会把欠载误判成结束
                if self.producer_finished.load(Ordering::Acquire) && receiver.is_empty() {
                    return None;
                }
                // 欠载时吐静音撑住输出流，绝不在这里阻塞
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

impl Source for LockFreeSource {
    /// 恒返回 `None`：本源跨度未知，rodio 因此不会按跨度做重采样 bootstrap ——
    /// 必须在交给 rodio 之前显式把采样率对齐到 mixer（见 `rodio_backend.rs`）。
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        // 畸形文件头可能写 0 声道；`NonZeroU16::new(0)` 返回 None，那时退到立体声。
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
        // 解码线程每 256 个样本查一次，正常路径上靠它退出。
        self.stop_flag.store(true, Ordering::Relaxed);
        drop(self.receiver.take());
    }
}
