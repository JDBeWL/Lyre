//! 重采样之后的声道布局转换。
//! 放在 `Source` 层而不是解码器里 —— LFE 低通的状态要跨帧，包边界重启滤波器会听出来。

use std::time::Duration;

use rodio::Source;
use rodio::{ChannelCount, SampleRate};

use super::dsp::{self, LfeConfig, LfeLowPass};

/// 把任意声道数/布局的音源转成输出设备要的声道数。
pub struct ChannelConvertSource<S> {
    inner: S,
    source_channels: usize,
    target_channels: usize,
    config: LfeConfig,
    /// 只在「源有独立 LFE，且要下混到不超过两声道」时才需要。
    lfe: Option<LfeLowPass>,
    /// 复用的输入帧与输出帧，逐帧处理，不在回调线程上分配。
    frame: Vec<f32>,
    out: Vec<f32>,
    out_pos: usize,
}

impl<S: Source<Item = f32>> ChannelConvertSource<S> {
    pub fn new(inner: S, target_channels: ChannelCount, config: LfeConfig) -> Self {
        let source_channels = inner.channels().get() as usize;
        let target_channels = target_channels.get() as usize;
        let layout = dsp::ChannelLayout::from_count(source_channels);

        let lfe = if layout.has_lfe() && target_channels <= 2 {
            Some(LfeLowPass::new(config.cutoff_hz, inner.sample_rate().get()))
        } else {
            None
        };

        Self {
            inner,
            source_channels,
            target_channels,
            config,
            lfe,
            frame: Vec::with_capacity(source_channels),
            // 空着起手：`out_pos < out.len()` 因此为假，第一次 `next` 会去填一帧
            out: Vec::new(),
            out_pos: 0,
        }
    }
}

impl<S: Source<Item = f32>> Iterator for ChannelConvertSource<S> {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        if self.out_pos < self.out.len() {
            let sample = self.out[self.out_pos];
            self.out_pos += 1;
            return Some(sample);
        }

        // 源在一帧中途结束就丢掉这半帧：宁可少一帧，也不让后续声道错位
        self.frame.clear();
        for _ in 0..self.source_channels {
            self.frame.push(self.inner.next()?);
        }

        self.out.resize(self.target_channels, 0.0);
        dsp::convert_frame(&self.frame, &mut self.out, self.lfe.as_mut(), self.config);
        self.out_pos = 1;
        self.out.first().copied()
    }
}

impl<S: Source<Item = f32>> Source for ChannelConvertSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        std::num::NonZeroU16::new(self.target_channels as u16).unwrap_or(ChannelCount::MIN)
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZero;

    struct Frame {
        samples: Vec<f32>,
        channels: u16,
    }

    impl Iterator for Frame {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            if self.samples.is_empty() {
                None
            } else {
                Some(self.samples.remove(0))
            }
        }
    }

    impl Source for Frame {
        fn current_span_len(&self) -> Option<usize> {
            None
        }

        fn channels(&self) -> ChannelCount {
            NonZero::new(self.channels).unwrap_or(ChannelCount::MIN)
        }

        fn sample_rate(&self) -> SampleRate {
            NonZero::new(48_000).unwrap_or(SampleRate::MIN)
        }

        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    fn source(samples: Vec<f32>, channels: u16) -> Frame {
        Frame { samples, channels }
    }

    fn channels(count: u16) -> ChannelCount {
        NonZero::new(count).unwrap_or(ChannelCount::MIN)
    }

    #[test]
    fn reports_the_target_channel_count() {
        let converted = ChannelConvertSource::new(
            source(vec![0.5, -0.4], 2),
            channels(6),
            LfeConfig::default(),
        );
        assert_eq!(converted.channels(), channels(6));
        assert_eq!(converted.sample_rate().get(), 48_000);
    }

    #[test]
    fn two_frames_of_surround_come_out_as_two_frames_of_stereo() {
        let mut samples = Vec::new();
        for frame in 0..2 {
            let base = frame as f32 * 0.1;
            samples.extend_from_slice(&[0.5 + base, -0.4 - base, 0.2, 0.9, 0.0, 0.0]);
        }
        let converted =
            ChannelConvertSource::new(source(samples, 6), channels(2), LfeConfig::default());

        let out: Vec<f32> = converted.collect();
        assert_eq!(out.len(), 4);
        let expected = (0.5 + 0.2 * 0.707) * 0.707;
        assert!((out[0] - expected).abs() < 1e-5, "第一帧 L = {}", out[0]);
    }

    #[test]
    fn a_partial_trailing_frame_is_dropped_rather_than_misaligned() {
        let converted = ChannelConvertSource::new(
            source(vec![0.5, -0.4, 0.2, 0.0, 0.0, 0.0, 0.9], 6),
            channels(2),
            LfeConfig::default(),
        );
        assert_eq!(converted.count(), 2);
    }

    #[test]
    fn identical_layouts_do_not_touch_the_samples() {
        let converted = ChannelConvertSource::new(
            source(vec![0.5, -0.4, 0.2, 0.9, 0.1, 0.2], 6),
            channels(6),
            LfeConfig::default(),
        );
        let out: Vec<f32> = converted.collect();
        assert_eq!(out, vec![0.5, -0.4, 0.2, 0.9, 0.1, 0.2]);
    }
}
