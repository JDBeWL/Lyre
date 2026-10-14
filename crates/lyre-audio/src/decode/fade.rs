//! 增益包络：起播与 seek 后的淡入，以及 stop、切歌、换设备时的淡出。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use rodio::Source;
use rodio::{ChannelCount, SampleRate};

/// 跨线程可写的目标增益。控制线程写，音频回调线程按帧逼近 —— `Player::set_volume`
/// 是阶跃的，直接改会爆音，所以淡出必须走这里。
#[derive(Debug)]
pub struct GainControl {
    target_bits: AtomicU32,
}

impl GainControl {
    /// 目标默认为满增益，新链路从静音爬到它。
    pub fn new() -> Self {
        Self {
            target_bits: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    /// 设置目标增益，会夹到 0..=1。
    pub fn set_target(&self, target: f32) {
        self.target_bits
            .store(target.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn target(&self) -> f32 {
        f32::from_bits(self.target_bits.load(Ordering::Relaxed))
    }
}

impl Default for GainControl {
    fn default() -> Self {
        Self::new()
    }
}

/// 在音源外面套一层增益包络。
pub struct GainRamp<S> {
    inner: S,
    control: Arc<GainControl>,
    channels: usize,
    /// 每帧允许的增益变化量，由 `ramp` 折算。
    step: f32,
    gain: f32,
    channel_index: usize,
}

impl<S> GainRamp<S> {
    /// 新链路一律从静音起，在 `ramp` 之内爬到控制线程当前的目标值。
    pub fn new(
        inner: S,
        ramp: Duration,
        sample_rate: u32,
        channels: u16,
        control: Arc<GainControl>,
    ) -> Self {
        let frames = (ramp.as_secs_f64() * f64::from(sample_rate)).round();
        // 时长为零的 ramp 等于不淡，直接给到目标值
        if frames < 1.0 {
            let gain = control.target();
            return Self {
                inner,
                control,
                channels: channels.max(1) as usize,
                step: 1.0,
                gain,
                channel_index: 0,
            };
        }

        Self {
            inner,
            control,
            channels: channels.max(1) as usize,
            step: (1.0 / frames) as f32,
            gain: 0.0,
            channel_index: 0,
        }
    }
}

impl<S: Iterator<Item = f32>> Iterator for GainRamp<S> {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;
        let output = sample * self.gain;

        // 增益按帧推进而非逐样本：同一帧内左右声道必须同增益，否则淡入期间声像会偏
        self.channel_index += 1;
        if self.channel_index >= self.channels {
            self.channel_index = 0;
            self.gain = approach(self.gain, self.control.target(), self.step);
        }

        Some(output)
    }
}

impl<S: Source<Item = f32>> Source for GainRamp<S> {
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

/// 朝目标走一步，不越过。
#[inline]
fn approach(current: f32, target: f32, step: f32) -> f32 {
    if target > current {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZero;

    /// 固定长度的 f32 音源，每个样本都是 1.0，于是输出就等于增益本身
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
            NonZero::new(2).unwrap_or(ChannelCount::MIN)
        }

        fn sample_rate(&self) -> SampleRate {
            NonZero::new(1000).unwrap_or(SampleRate::MIN)
        }

        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    fn ramp(len: usize, ramp: Duration, channels: u16) -> (GainRamp<Ramp>, Arc<GainControl>) {
        let control = Arc::new(GainControl::new());
        let source = GainRamp::new(Ramp(len), ramp, 1000, channels, Arc::clone(&control));
        (source, control)
    }

    #[test]
    fn gain_ramps_per_frame_not_per_sample() {
        // 1000Hz、立体声、淡入 2ms = 2 帧。4 个样本（2 帧）应当得到 0,0 / 0.5,0.5
        let (source, _control) = ramp(4, Duration::from_millis(2), 2);
        let faded: Vec<f32> = source.collect();

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
        let (source, _control) = ramp(10, Duration::from_millis(1), 2);
        let faded: Vec<f32> = source.collect();

        // 1ms @ 1000Hz = 1 帧淡入，之后应当全为 1.0
        assert_eq!(faded[0], 0.0);
        assert_eq!(faded[1], 0.0);
        for (index, sample) in faded.iter().enumerate().skip(2) {
            assert_eq!(*sample, 1.0, "第 {index} 个样本应当已满增益");
        }
    }

    #[test]
    fn zero_duration_fade_is_a_passthrough() {
        let (source, _control) = ramp(4, Duration::ZERO, 2);
        assert!(source.collect::<Vec<f32>>().iter().all(|s| *s == 1.0));
    }

    #[test]
    fn shorter_than_fade_source_stays_below_full_gain() {
        // 音源比淡入窗口还短，不应越界或 panic，只是整个都在淡入段里
        let (source, _control) = ramp(2, Duration::from_millis(100), 2);
        let faded: Vec<f32> = source.collect();

        assert_eq!(faded.len(), 2);
        assert_eq!(faded[0], 0.0);
        assert_eq!(faded[1], 0.0);
    }

    #[test]
    fn pulling_the_target_back_to_zero_fades_the_stream_out() {
        let (mut source, control) = ramp(8, Duration::from_millis(2), 1);

        // 2 帧淡入到满增益
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.next(), Some(0.5));

        // 满增益那一帧之后把目标拉回 0，两帧内归零
        control.set_target(0.0);
        assert_eq!(source.next(), Some(1.0), "改目标之前的那一帧仍是满增益");
        assert_eq!(source.next(), Some(0.5), "随后按同样的步长回落到 0");
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.next(), Some(0.0), "该停在 0 而不是继续往下穿");
    }

    #[test]
    fn gain_never_overshoots_the_target() {
        // 目标比一步的行程还小：应当一步落到目标，而不是冲过去再退回来
        let control = Arc::new(GainControl::new());
        control.set_target(0.4);
        let mut source = GainRamp::new(
            Ramp(4),
            Duration::from_millis(1),
            1000,
            1,
            Arc::clone(&control),
        );

        assert_eq!(source.next(), Some(0.0), "第一帧仍是起点");
        assert_eq!(source.next(), Some(0.4));
        assert_eq!(source.next(), Some(0.4));
    }
}
