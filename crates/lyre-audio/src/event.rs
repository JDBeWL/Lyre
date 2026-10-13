use std::path::PathBuf;
use std::time::Duration;

/// 播放状态。`as_u8` / `from_u8` 的数值映射会存进原子量。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PlaybackState {
    /// 还没有装载任何曲目或音频设备不可用
    #[default]
    Idle,
    /// 已装载但停在开头
    Paused,
    Playing,
    Stopped,
}

impl PlaybackState {
    pub const fn as_u8(self) -> u8 {
        match self {
            PlaybackState::Idle => 0,
            PlaybackState::Paused => 1,
            PlaybackState::Playing => 2,
            PlaybackState::Stopped => 3,
        }
    }

    pub const fn from_u8(value: u8) -> Self {
        match value {
            1 => PlaybackState::Paused,
            2 => PlaybackState::Playing,
            3 => PlaybackState::Stopped,
            _ => PlaybackState::Idle,
        }
    }

    pub const fn is_playing(self) -> bool {
        matches!(self, PlaybackState::Playing)
    }
}

/// 音频线程到 UI 的事件，只承载语义变化；高频进度走 [`crate::Snapshot`]，否则一秒 20 条消息会打爆 UI 通知。
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerEvent {
    /// 曲目装载完成，附带解码出的时长。
    Loaded {
        path: PathBuf,
        duration: Duration,
    },
    State(PlaybackState),
    /// 一首歌自然播放结束。由 UI 决定下一曲还是循环。
    Ended,
    Volume(f32),
    /// 音频链路出错（设备打不开、文件解不了码……）。
    Error(String),
}
