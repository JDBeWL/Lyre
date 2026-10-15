//! 上次播放的位置落盘到独立的 `resume.json`，只在 `Settings::resume_position` 打开时读写。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::persist;

const FILE: &str = "resume.json";

/// 存路径而不是 `TrackId`：`TrackId` 由路径算出，重新导入会变，路径才是跨启动稳定的键。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resume {
    pub path: PathBuf,
    pub position_ms: u64,
}

impl Resume {
    pub fn new(path: impl Into<PathBuf>, position_ms: u64) -> Self {
        Self {
            path: path.into(),
            position_ms,
        }
    }

    /// 读不到（首次启动、文件损坏）就当没有。
    pub fn load() -> Option<Self> {
        persist::load_or_none(FILE)
    }

    /// 写回磁盘。失败只记日志，不影响调用方。
    pub fn save(&self) {
        persist::save_lossy(FILE, self);
    }

    /// 关掉设置项时清掉，否则下次再开会复活一个几个月前的位置。
    pub fn clear() {
        persist::remove_lossy(FILE);
    }
}

#[cfg(test)]
mod tests {
    use super::Resume;
    use std::path::Path;

    #[test]
    fn round_trips_through_json() {
        let resume = Resume::new(Path::new("D:/音乐/a.flac"), 83_000);
        let text = serde_json::to_string(&resume).expect("序列化不该失败");
        let back: Resume = serde_json::from_str(&text).expect("反序列化不该失败");
        assert_eq!(back, resume);
    }

    #[test]
    fn a_corrupt_file_reads_as_nothing() {
        let back: Result<Resume, _> = serde_json::from_str("这不是 JSON");
        assert!(back.is_err(), "坏文件应当解析失败，`load` 据此退回 None");
    }
}
