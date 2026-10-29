use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("读取文件失败 {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("解析音频标签失败 {path}: {source}")]
    Tag {
        path: PathBuf,
        #[source]
        source: lofty::error::FileParseError,
    },
}

pub type Result<T> = std::result::Result<T, CoreError>;
