//! 持久化错误。

use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    /// 找不到可用的数据目录（`APPDATA` / `HOME` 等环境变量缺失）。
    #[error("没有可用的数据目录")]
    NoDataDir,

    /// 文件读写失败。
    #[error("读写 {} 失败：{source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// 序列化 / 反序列化失败。
    #[error("JSON 处理失败：{0}")]
    Json(#[from] serde_json::Error),
}
