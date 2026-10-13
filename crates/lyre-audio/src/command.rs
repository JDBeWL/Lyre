use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum PlayerCommand {
    /// 装载一首曲目。`autoplay` 为假时装载后停在开头。
    Load {
        path: PathBuf,
        autoplay: bool,
    },
    Play,
    Pause,
    TogglePlay,
    Stop,
    /// 跳转到指定位置。只在用户松开进度条时下发一次 —— rodio 的 seek 会重建解码器，连续调用会卡顿爆音。
    Seek(Duration),
    SetVolume(f32),
    /// 让音频线程退出。由 `AudioHandle::drop` 发出。
    Shutdown,
}
