//! 把解码链重采样到独占输出协商到的采样率。
//! 只在两者不一致时用到：格式协商优先要声源自己的采样率，对上了就是位完美直出，这一步整个省掉。

use std::time::Duration;

use audioadapter_buffers::direct::InterleavedSlice;
use rodio::Source;
use rodio::{ChannelCount, SampleRate};
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use crate::error::{ExclusiveError, Result};

/// 滤波器长度，256 是 rubato 建议的起点。
const SINC_LEN: usize = 256;
/// 保留到奈奎斯特的 92.5%，给抗镜像留一点过渡带。
const CUTOFF: f32 = 0.925;
/// 插值中间点数量，128 是常用档。
const OVERSAMPLING: usize = 128;

/// 每次递交的输入块长度（帧），约 21ms @48k。
/// 源耗尽之后多喂几块静音（块数由重采样器的延迟算出来，见 [`RubatoResampleSource::new`]）。
const fn chunk_frames(sample_rate: u32) -> usize {
    match sample_rate {
        0..=32_000 => 512,
        32_001..=64_000 => 1024,
        64_001..=128_000 => 2048,
        _ => 4096,
    }
}

/// 喂料状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Feed {
    /// 正常拉取源数据
    Data,
    /// 源已耗尽，还剩 n 块静音要喂
    Drain(usize),
    /// 全部吐完
    Done,
}

/// 把源重采样到 `target_rate`，声道数保持不变。
pub struct RubatoResampleSource<S> {
    inner: S,
    target_rate: SampleRate,
    channels: usize,
    frames_per_chunk: usize,
    resampler: Async<f32>,
    feed: Feed,
    ratio: f64,
    input: Vec<f32>,
    output: Vec<f32>,
    position: usize,
    produced: usize,
    /// 已经从源里读出来的真实帧数（不含补的静音）。
    fed_frames: usize,
    /// 源读完之后按比例算出的目标总帧数。到这儿就收工 —— 补齐的静音不该算进时长。
    total_frames: Option<usize>,
    /// 已经吐出去的帧数。
    out_frames: usize,
    /// 源读完之后还要喂几块静音，由重采样器的延迟算出来。
    flush_chunks: usize,
}

impl<S: Source<Item = f32>> RubatoResampleSource<S> {
    pub fn new(inner: S, target_rate: SampleRate) -> Result<Self> {
        let channels = inner.channels().get() as usize;
        let source_rate = inner.sample_rate().get();
        let frames_per_chunk = chunk_frames(source_rate);

        let parameters =
            SincInterpolationParameters::new(SINC_LEN, WindowFunction::BlackmanHarris2)
                .f_cutoff(CUTOFF)
                .interpolation(SincInterpolationType::Linear)
                .oversampling_factor(OVERSAMPLING);

        let ratio = f64::from(target_rate.get()) / f64::from(source_rate);
        let resampler = Async::<f32>::new_sinc(
            ratio,
            2.0,
            &parameters,
            frames_per_chunk,
            channels,
            FixedAsync::Input,
        )
        .map_err(|err| ExclusiveError::Resampler(err.to_string()))?;

        let output_frames = resampler.output_frames_max();
        // 重采样器有固定启动延迟：头几块只填缓冲、不吐输出帧。延迟按输出帧计，折回输入帧算块数。
        // 少喂会把曲尾切掉；多喂的静音由 `total_frames` 截断，不会多交出去。
        let flush_chunks = {
            let latency_frames = (resampler.output_delay() as f64 / ratio).ceil() as usize;
            latency_frames.div_ceil(frames_per_chunk) + 1
        };

        Ok(Self {
            inner,
            target_rate,
            channels,
            frames_per_chunk,
            resampler,
            feed: Feed::Data,
            ratio,
            input: vec![0.0; frames_per_chunk * channels],
            output: vec![0.0; output_frames * channels],
            position: 0,
            produced: 0,
            fed_frames: 0,
            total_frames: None,
            out_frames: 0,
            flush_chunks,
        })
    }
}

impl<S: Iterator<Item = f32>> RubatoResampleSource<S> {
    fn samples_per_chunk(&self) -> usize {
        self.frames_per_chunk * self.channels
    }

    /// 喂一块进去，返回这一轮产出的样本数；`None` 表示源与滤波器都已吐完。
    fn pump(&mut self) -> Option<usize> {
        let want = self.samples_per_chunk();
        self.input.clear();

        match self.feed {
            Feed::Done => return None,
            Feed::Data => {
                let mut short = false;
                while self.input.len() < want {
                    let Some(sample) = self.inner.next() else {
                        short = true;
                        break;
                    };
                    self.input.push(sample);
                }

                // 这一块真正从源里读到的帧数。整块也要算进来 —— 只在读到 None 时才累加
                // 会把前面每一块都漏掉，总帧数算出来比已产出的还小，尾巴会被整段截掉。
                self.fed_frames += self.input.len() / self.channels;

                if short {
                    // 源到头了：这一块补静音凑满，之后还要再喂几块把滤波器里的尾巴推出来
                    self.total_frames = Some((self.fed_frames as f64 * self.ratio).ceil() as usize);
                    self.input.resize(want, 0.0);
                    self.feed = Feed::Drain(self.flush_chunks);
                }
            }
            Feed::Drain(left) => {
                self.input.resize(want, 0.0);
                self.feed = if left <= 1 {
                    Feed::Done
                } else {
                    Feed::Drain(left - 1)
                };
            }
        }

        let input =
            InterleavedSlice::new(&self.input, self.channels, self.frames_per_chunk).ok()?;
        let frames_out = self.output.len() / self.channels;
        let mut output =
            InterleavedSlice::new_mut(&mut self.output, self.channels, frames_out).ok()?;

        let (_, written) = self
            .resampler
            .process_into_buffer(&input, &mut output, None)
            .ok()?;

        // 源读完之后，剩下的尾巴只留到按比例应有的长度，补齐的静音不交出去
        let produced = match self.total_frames {
            Some(total) => {
                let left = total.saturating_sub(self.out_frames).min(written);
                if left == 0 {
                    self.feed = Feed::Done;
                    return None;
                }
                left
            }
            None => written,
        };
        self.out_frames += produced;
        Some(produced * self.channels)
    }
}

impl<S: Iterator<Item = f32>> Iterator for RubatoResampleSource<S> {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        loop {
            if self.position < self.produced {
                let sample = self.output[self.position];
                self.position += 1;
                return Some(sample);
            }

            self.produced = self.pump()?;
            self.position = 0;
            // 滤波器有启动延迟，头几轮可能就是吐不出东西；继续喂，喂到出为止
        }
    }
}

impl<S: Source<Item = f32>> Source for RubatoResampleSource<S> {
    /// 块边界由内部决定，报给 mixer 只会让它误判跨度，恒为 `None`。
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.target_rate
    }

    /// 重采样之后精确时长不再可得（还有滤波器延迟与补齐的尾巴），宁可不报。
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;
    use std::num::NonZero;

    /// 定长正弦源。
    struct Tone {
        left: usize,
        channels: usize,
        rate: u32,
        phase: f32,
        step: f32,
    }

    impl Tone {
        fn new(frames: usize, channels: usize, rate: u32, hz: f32) -> Self {
            Self {
                left: frames * channels,
                channels,
                rate,
                phase: 0.0,
                step: TAU * hz / rate as f32,
            }
        }
    }

    impl Iterator for Tone {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            if self.left == 0 {
                return None;
            }
            let value = self.phase.sin() * 0.5;
            self.left -= 1;
            if self.left.is_multiple_of(self.channels) {
                self.phase = (self.phase + self.step) % TAU;
            }
            Some(value)
        }
    }

    impl Source for Tone {
        fn current_span_len(&self) -> Option<usize> {
            None
        }

        fn channels(&self) -> ChannelCount {
            NonZero::new(self.channels as u16).unwrap_or(ChannelCount::MIN)
        }

        fn sample_rate(&self) -> SampleRate {
            NonZero::new(self.rate).unwrap_or(SampleRate::MIN)
        }

        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    fn rate(value: u32) -> SampleRate {
        NonZero::new(value).unwrap_or(SampleRate::MIN)
    }

    #[test]
    fn upsampling_produces_about_the_right_number_of_samples() {
        const FRAMES: usize = 4_000;
        let source = Tone::new(FRAMES, 2, 8_000, 440.0);
        let resampled =
            RubatoResampleSource::new(source, rate(48_000)).expect("重采样器应当能建起来");

        assert_eq!(resampled.channels().get(), 2, "声道数不该被改动");
        assert_eq!(resampled.sample_rate().get(), 48_000);

        let out: Vec<f32> = resampled.collect();
        // 长度必须严格等于「真实帧数 × 倍率」：补齐的静音与滤波器的尾巴都不该多交出去
        let expected = FRAMES * 2 * 6;
        assert_eq!(out.len(), expected, "6 倍上采样的样本数不对");

        let peak = out.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()));
        assert!((0.4..0.55).contains(&peak), "峰值 {peak} 偏离了原始的 0.5");
        assert!(out.iter().all(|s| s.is_finite()), "不该出现 NaN / 无穷");
    }

    #[test]
    fn downsampling_also_keeps_the_amplitude() {
        let source = Tone::new(16_000, 2, 48_000, 440.0);
        let resampled =
            RubatoResampleSource::new(source, rate(44_100)).expect("重采样器应当能建起来");

        let out: Vec<f32> = resampled.collect();
        let expected = 16_000 * 2 * 44_100 / 48_000;
        assert_eq!(out.len(), expected, "44.1k 降采样的样本数不对");

        let peak = out.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()));
        assert!((0.4..0.55).contains(&peak), "峰值 {peak} 偏离了原始的 0.5");
    }

    /// 素材比一个数据块还短：不能卡住、不能 panic，也不能一个样本都不给。
    #[test]
    fn a_source_shorter_than_one_chunk_still_yields_samples() {
        let source = Tone::new(64, 1, 44_100, 1_000.0);
        let resampled =
            RubatoResampleSource::new(source, rate(48_000)).expect("重采样器应当能建起来");

        let out: Vec<f32> = resampled.collect();
        // 64 帧按 1.088 倍算出来是 70 帧，一块完整的输入都不够，仍然要给到这个数
        assert_eq!(out.len(), 70, "短素材的长度同样要按比例算");
        assert!(out.iter().all(|s| s.is_finite()), "不该出现 NaN / 无穷");
    }

    #[test]
    fn mono_stays_mono() {
        let source = Tone::new(2_000, 1, 22_050, 300.0);
        let resampled =
            RubatoResampleSource::new(source, rate(44_100)).expect("重采样器应当能建起来");
        assert_eq!(resampled.channels().get(), 1);
        let out: Vec<f32> = resampled.collect();
        assert_eq!(out.len(), 4_000, "2000 帧 2 倍上采样应当给 4000 帧");
    }
}
