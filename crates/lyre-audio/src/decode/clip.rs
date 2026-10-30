//! 末级软削波：多声道下混、音量与淡出叠加之后，输出电平可能超过满刻度。

use std::time::Duration;

use rodio::Source;
use rodio::{ChannelCount, SampleRate};

use super::dsp;

pub struct SoftClip<S> {
    inner: S,
}

impl<S> SoftClip<S> {
    pub const fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S: Iterator<Item = f32>> Iterator for SoftClip<S> {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        Some(dsp::soft_clip(self.inner.next()?))
    }
}

impl<S: Source<Item = f32>> Source for SoftClip<S> {
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
    use std::num::NonZero;

    struct Fixed(Vec<f32>);

    impl Iterator for Fixed {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            if self.0.is_empty() {
                None
            } else {
                Some(self.0.remove(0))
            }
        }
    }

    impl Source for Fixed {
        fn current_span_len(&self) -> Option<usize> {
            None
        }

        fn channels(&self) -> ChannelCount {
            NonZero::new(2).unwrap_or(ChannelCount::MIN)
        }

        fn sample_rate(&self) -> SampleRate {
            NonZero::new(48_000).unwrap_or(SampleRate::MIN)
        }

        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    #[test]
    fn keeps_the_in_range_samples_and_compresses_the_rest() {
        let clipped: Vec<f32> = SoftClip::new(Fixed(vec![0.5, 1.05, -1.2, 0.0])).collect();
        assert_eq!(clipped[0], 0.5, "0.5 在阈值内，必须原样");
        assert!((0.95..1.0).contains(&clipped[1]), "1.05 → {}", clipped[1]);
        assert!((-1.0..-0.95).contains(&clipped[2]), "-1.2 → {}", clipped[2]);
        assert_eq!(clipped[3], 0.0);
    }

    #[test]
    fn extreme_inputs_stop_at_full_scale_instead_of_overshooting() {
        let clipped: Vec<f32> = SoftClip::new(Fixed(vec![100.0, -100.0, f32::MAX])).collect();
        assert_eq!(clipped[0], 1.0);
        assert_eq!(clipped[1], -1.0);
        assert_eq!(clipped[2], 1.0);
    }

    #[test]
    fn forwards_the_source_metadata() {
        let clipped = SoftClip::new(Fixed(vec![0.0]));
        assert_eq!(clipped.channels().get(), 2);
        assert_eq!(clipped.sample_rate().get(), 48_000);
        assert_eq!(clipped.current_span_len(), None);
    }
}
