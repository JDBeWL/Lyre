//! 独占链路的错误类型。用户可见的文案都在这里，上层只透传 [`Display`](std::fmt::Display)。

/// 独占输出不可用或中途失效。
#[derive(Debug, thiserror::Error)]
pub enum ExclusiveError {
    /// 设备不接受任何可用的独占格式（被占用、驱动只支持独占之外的口径）。
    #[error("设备不接受独占格式：{0}")]
    Unsupported(String),

    /// COM 对象建不起来、流初始化失败。
    #[error("初始化独占输出失败：{0}")]
    Initialize(String),

    /// 渲染线程往设备写数据失败。
    #[error("独占输出写入失败：{0}")]
    Write(String),

    /// 重采样器建不起来。
    #[error("重采样器创建失败：{0}")]
    Resampler(String),

    /// 解码链路建不起来（文件打不开、格式不认识）。
    #[error("{0}")]
    Decode(String),

    /// 推送线程还没结束就失效了（切歌、设备切换）。
    #[error("独占输出已被取消")]
    Cancelled,

    #[error("音频线程创建失败：{0}")]
    ThreadSpawn(#[source] std::io::Error),
}

/// 解码链路的错误直接透传它的文案：那边已经有面向用户的写法了。
impl From<lyre_audio::AudioError> for ExclusiveError {
    fn from(err: lyre_audio::AudioError) -> Self {
        Self::Decode(err.to_string())
    }
}

/// 反向：独占链路的错误在 `lyre_audio` 那边只留一个入口，内部细节不摊到 `AudioBackend` 上。
impl From<ExclusiveError> for lyre_audio::AudioError {
    fn from(err: ExclusiveError) -> Self {
        Self::Exclusive(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ExclusiveError>;
