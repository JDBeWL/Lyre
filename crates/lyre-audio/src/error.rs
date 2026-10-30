//! 音频链路的错误类型：保留结构与 `#[source]` 链，面向用户的文案由 `Display` 提供。
//! `PlayerEvent::Error` 跨线程只带文案字符串 —— `std::io::Error` 既不 `Clone`
//! 也不 `PartialEq`，放进枚举会破坏 `PlayerEvent` 的派生。

/// 音频链路错误。
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// 打不开音频输出设备（无声卡、被占用、驱动异常）。
    #[error("无法打开音频输出设备：{0}")]
    DeviceOpen(String),

    #[error("无法打开文件：{0}")]
    FileOpen(#[source] std::io::Error),

    #[error("无法识别音频格式：{0}")]
    Probe(String),

    /// 认得出是什么格式，但引擎没有对应的解码实现。
    /// 与 [`Self::Probe`] 的区别是「认不出来」还是「认出来了但放不了」 —— 后者要给出格式名。
    #[error("不支持的音频格式：{0}")]
    Unsupported(String),

    /// 文件能打开，但一个样本都解不出来（只剩外壳、或编码不被支持）。
    /// 必须当错误上报：静默当成「放完了」会让这首一上来就被判成结束，列表循环时看着像无声快进。
    #[error("文件里没有可解码的音频数据")]
    NoAudioData,

    #[error("文件里没有音频轨")]
    NoAudioTrack,

    #[error("缺少音频编解码参数")]
    MissingCodecParams,

    #[error("解码器尚未初始化")]
    NotInitialized,

    #[error("无法创建解码器：{0}")]
    DecoderCreate(String),

    #[error("读取数据包失败：{0}")]
    ReadPacket(String),

    #[error("解码失败：{0}")]
    Decode(String),

    #[error("跳转失败：{0}")]
    Seek(String),

    #[error("缓冲预填充失败：{0}")]
    Prefill(String),

    #[error("音频线程创建失败：{0}")]
    ThreadSpawn(#[source] std::io::Error),

    /// 独占输出不可用（设备不支持、被占用、格式协商失败）。
    /// 文案由独占后端给出，这里只做透传。
    #[error("{0}")]
    Exclusive(String),
}

pub type Result<T> = std::result::Result<T, AudioError>;

#[cfg(test)]
mod tests {
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
        assert_eq!(
            AudioError::Unsupported("WavPack".to_owned()).to_string(),
            "不支持的音频格式：WavPack"
        );
        assert_eq!(
            AudioError::NoAudioData.to_string(),
            "文件里没有可解码的音频数据"
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
