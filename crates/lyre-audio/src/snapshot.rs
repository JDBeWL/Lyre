use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use crate::event::PlaybackState;

/// 播放状态的无锁快照：音频线程每 50ms 写一次，渲染线程随时读，双方都不加锁。
/// 进度条每秒读几十次，改成发消息的话光通知开销就足以掉帧。
#[derive(Debug)]
pub struct Snapshot {
    position_ms: AtomicU64,
    duration_ms: AtomicU64,
    state: AtomicU8,
    /// `f32::to_bits` 的结果
    volume_bits: AtomicU32,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            position_ms: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            state: AtomicU8::new(PlaybackState::Idle.as_u8()),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
        }
    }
}

impl Snapshot {
    pub fn position(&self) -> Duration {
        Duration::from_millis(self.position_ms.load(Ordering::Relaxed))
    }

    pub fn set_position(&self, value: Duration) {
        self.position_ms
            .store(value.as_millis() as u64, Ordering::Relaxed);
    }

    pub fn duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms.load(Ordering::Relaxed))
    }

    pub fn set_duration(&self, value: Duration) {
        self.duration_ms
            .store(value.as_millis() as u64, Ordering::Relaxed);
    }

    pub fn state(&self) -> PlaybackState {
        PlaybackState::from_u8(self.state.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, value: PlaybackState) {
        self.state.store(value.as_u8(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, value: f32) {
        self.volume_bits
            .store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// 已播比例，`0.0..=1.0`。时长未知时返回 0。
    pub fn progress(&self) -> f32 {
        let total = self.duration().as_secs_f32();
        if total <= 0.0 {
            return 0.0;
        }
        (self.position().as_secs_f32() / total).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_state_through_atomic() {
        let s = Snapshot::default();
        assert_eq!(s.state(), PlaybackState::Idle);
        s.set_state(PlaybackState::Playing);
        assert_eq!(s.state(), PlaybackState::Playing);
    }

    #[test]
    fn volume_round_trips_and_clamps() {
        let s = Snapshot::default();
        assert_eq!(s.volume(), 1.0, "默认音量应当是满格");
        s.set_volume(0.42);
        assert!((s.volume() - 0.42).abs() < f32::EPSILON);
        s.set_volume(9.0);
        assert_eq!(s.volume(), 1.0, "音量需要被夹在合法区间");
    }

    #[test]
    fn progress_handles_unknown_duration() {
        let s = Snapshot::default();
        assert_eq!(s.progress(), 0.0, "时长未知时不应产生 NaN");
        s.set_duration(Duration::from_secs(200));
        s.set_position(Duration::from_secs(50));
        assert!((s.progress() - 0.25).abs() < 1e-6);
    }
}
