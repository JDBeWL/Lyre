//! 起播与 seek 后的淡入

use std::time::Duration;

use rodio::Source;
use rodio::{ChannelCount, SampleRate};

/// 在音源外面套一段线性淡入。
pub struct FadeIn<S> {
    inner: S,
    channels: usize,
    total_frames: u64,
    frame: u64,
    channel_index: usize,
}

impl<S> FadeIn<S> {
    pub fn new(inner: S, duration: Duration, sample_rate: u32, channels: u16) -> Self {
        let total_frames = (duration.as_secs_f64() * f64::from(sample_rate)).round() as u64;
        Self {
            inner,
            channels: channels.max(1) as usize,
            total_frames,
            frame: 0,
            channel_index: 0,
        }
    }
}

impl<S: Iterator<Item = f32>> Iterator for FadeIn<S> {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;

        // 增益按帧推进而非样本，逐样本递增会让同帧左右增益不同，淡入期间声像偏一边
        let gain = if self.total_frames == 0 || self.frame >= self.total_frames {
            1.0
        } else {
            self.frame as f32 / self.total_frames as f32
        };

        self.channel_index += 1;
        if self.channel_index >= self.channels {
            self.channel_index = 0;
            self.frame += 1;
        }

        Some(sample * gain)
    }
}

impl<S> Source for FadeIn<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
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

    /// 构造一个固定长度的 f32 音源用于测试
    struct Ramp(usize);
    impl Iterator for Ramp {
        type Item = f32;
        fn next(&mut self) -> Option<f32> {
            if self.0 == 0 {
                None
            } else {
                self.0 -= 1;
                Some(1.0)
            }
        }
    }
    impl Source for Ramp {
        fn current_span_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> ChannelCount {
            std::num::NonZero::new(2).unwrap()
        }
        fn sample_rate(&self) -> SampleRate {
            std::num::NonZero::new(1000).unwrap()
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    #[test]
    fn gain_ramps_per_frame_not_per_sample() {
        // 1000Hz、立体声、淡入 2ms = 2 帧。4 个样本（2 帧）应当得到 0,0 / 0.5,0.5
        let faded: Vec<f32> = FadeIn::new(Ramp(4), Duration::from_millis(2), 1000, 2).collect();
        assert_eq!(faded.len(), 4);
        assert_eq!(faded[0], 0.0);
        assert_eq!(faded[1], 0.0, "同一帧内左右声道增益必须相同");
        assert!((faded[2] - 0.5).abs() < 1e-6);
        assert!(
            (faded[3] - 0.5).abs() < 1e-6,
            "同一帧内左右声道增益必须相同"
        );
    }

    #[test]
    fn gain_reaches_full_after_the_fade_window() {
        let faded: Vec<f32> = FadeIn::new(Ramp(10), Duration::from_millis(1), 1000, 2).collect();
        // 1ms @ 1000Hz = 1 帧淡入，之后应当全为 1.0
        assert_eq!(faded[0], 0.0);
        assert_eq!(faded[1], 0.0);
        for (i, sample) in faded.iter().enumerate().skip(2) {
            assert_eq!(*sample, 1.0, "第 {i} 个样本应当已满增益");
        }
    }

    #[test]
    fn zero_duration_fade_is_a_passthrough() {
        let faded: Vec<f32> = FadeIn::new(Ramp(4), Duration::ZERO, 1000, 2).collect();
        assert!(faded.iter().all(|s| *s == 1.0));
    }

    #[test]
    fn shorter_than_fade_source_stays_below_full_gain() {
        // 音源比淡入窗口还短，不应越界或 panic，只是整个都在淡入段里
        let faded: Vec<f32> = FadeIn::new(Ramp(2), Duration::from_millis(100), 1000, 2).collect();
        assert_eq!(faded.len(), 2);
        assert_eq!(faded[0], 0.0);
        assert_eq!(faded[1], 0.0);
    }
}
