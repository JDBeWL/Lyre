//! 音频链路的错误类型，保留结构、支持 `#[source]` 链，让上层能按类别判断，
//! 而不是只能拿到一串给用户看的文字。
//!
//! 面向用户的文案由 [`Display`](std::fmt::Display) 提供；跨线程只传它的字符串形式
//! （[`PlayerEvent::Error`](crate::PlayerEvent::Error) 仍是 `String`，因为 `std::io::Error`
//! 既不 `Clone` 也不 `PartialEq`，会破坏 `PlayerEvent` 的派生）。

/// 音频链路错误。
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// 打不开音频输出设备（无声卡、被占用、驱动异常）。
    #[error("无法打开音频输出设备：{0}")]
    DeviceOpen(String),

    /// 打不开媒体文件。
    #[error("无法打开文件：{0}")]
    FileOpen(#[source] std::io::Error),

    /// 探测容器 / 编码格式失败。
    #[error("无法识别音频格式：{0}")]
    Probe(String),

    /// 文件里没有音频轨。
    #[error("文件里没有音频轨")]
    NoAudioTrack,

    /// 缺少解码必需的能力参数（采样率 / 声道数等）。
    #[error("缺少音频编解码参数")]
    MissingCodecParams,

    /// 解码器尚未初始化（内部不变量被破坏）。
    #[error("解码器尚未初始化")]
    NotInitialized,

    /// 创建解码器失败。
    #[error("无法创建解码器：{0}")]
    DecoderCreate(String),

    /// 读取数据包失败。
    #[error("读取数据包失败：{0}")]
    ReadPacket(String),

    /// 解码失败。
    #[error("解码失败：{0}")]
    Decode(String),

    /// 跳转失败。
    #[error("跳转失败：{0}")]
    Seek(String),

    /// 缓冲预填充失败。
    #[error("缓冲预填充失败：{0}")]
    Prefill(String),

    /// 音频线程创建失败。
    #[error("音频线程创建失败：{0}")]
    ThreadSpawn(#[source] std::io::Error),
}

/// 音频链路统一 `Result`。
pub type Result<T> = std::result::Result<T, AudioError>;

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：本 crate 无同名 test 宏，但保持与全项目一致的风格。
    use super::AudioError;

    #[test]
    fn display_carries_the_user_facing_text() {
        assert_eq!(AudioError::NoAudioTrack.to_string(), "文件里没有音频轨");
        assert_eq!(
            AudioError::DeviceOpen("没有输出设备".to_owned()).to_string(),
            "无法打开音频输出设备：没有输出设备"
        );
        assert_eq!(
            AudioError::Decode("位深不支持".to_owned()).to_string(),
            "解码失败：位深不支持"
        );
    }

    #[test]
    fn io_errors_keep_their_source_chain() {
        use std::error::Error as _;
        let err = AudioError::FileOpen(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file",
        ));
        assert!(err.source().is_some(), "io 错误应保留 #[source] 链");
    }
}
