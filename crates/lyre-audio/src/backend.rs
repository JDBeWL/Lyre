use std::path::Path;
use std::time::Duration;

use crate::error::Result;

/// 音频后端的抽象边界。实现者可以是 `!Send` 的，只在音频线程内部构造和使用。
pub trait AudioBackend: 'static {
    /// 装载曲目。返回解码出的时长，未知时返回 `Duration::ZERO`。
    fn load(&mut self, path: &Path, autoplay: bool) -> Result<Duration>;

    /// 开始（或继续）播放当前曲目。重播是按当前曲目重建音源链路，可能失败，
    /// 返回 `Err` 时上层必须把状态退回「未在播放」并显示错误。
    fn play(&mut self) -> Result<()>;

    fn pause(&mut self);

    fn stop(&mut self);

    fn seek(&mut self, position: Duration) -> Result<()>;

    fn set_volume(&mut self, volume: f32);

    fn position(&self) -> Duration;

    /// 当前队列是否已经放空 —— 用来判定「一首歌自然播完了」。
    fn is_finished(&self) -> bool;

    /// 切换输出设备，`None` 表示跟随系统默认。正在播放的曲目要在新设备上接着放。
    /// 默认无操作，注入式假后端不必实现。
    fn set_device(&mut self, device: Option<&str>) -> Result<()> {
        let _ = device;
        Ok(())
    }

    /// 当前输出设备的稳定标识；跟随系统默认或未知时返回 `None`。
    fn current_device(&self) -> Option<String> {
        None
    }
}
