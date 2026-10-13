//! 曲库缓存，落盘到 `library.json`：读不到退回空、写不进只记日志。
//! 文件形状是按导入顺序排列的裸 `Vec<Track>`，不含排序键与播放模式。

use lyre_core::Track;

use crate::persist;

/// 落盘文件名。
const FILE: &str = "library.json";

/// 读缓存并剔除已不存在的文件（不剔除会留着点不动的幽灵行）。
pub fn load() -> Vec<Track> {
    prune_missing(persist::load_or_default(FILE))
}

/// 写回磁盘。失败只记日志 —— `%APPDATA%` 只读不该让播放器打不开。
/// 用紧凑排版：整库条目可能上千，缩进会让文件涨好几倍。
pub fn save(tracks: &impl serde::Serialize) {
    persist::save_compact_lossy(FILE, tracks);
}

/// 剔除盘上已不存在的曲目。
/// 一个都不在时要原样保留：那是盘没醒 / 可移动盘没挂载，照常剔除会把整库覆盖成空。
fn prune_missing(tracks: Vec<Track>) -> Vec<Track> {
    if !tracks.is_empty() && tracks.iter().all(|track| !track.path.is_file()) {
        return tracks;
    }
    tracks
        .into_iter()
        .filter(|track| track.path.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use lyre_core::{AudioInfo, Track, TrackId};

    /// 落盘形状一旦漂移，老用户的曲库就读不回来。
    #[test]
    fn 落盘是裸数组且带导入顺序语义() {
        let tracks = vec![track("/m/a.flac"), track("/m/b.mp3")];
        let text = serde_json::to_string(&tracks).unwrap();
        assert!(text.starts_with("[{\""), "顶层必须是数组：{text}");

        let back: Vec<Track> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, tracks);
    }

    /// 老文件里没有 `audio` 字段，必须照常读得出来。
    #[test]
    fn 缺字段的旧条目退默认值() {
        let old = r#"[{"id":7,"path":"D:/m/x.mp3","title":"旧条目","artist":null,
                       "album":null,"duration":null,"has_cover":false}]"#;
        let parsed: Vec<Track> = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "旧条目");
        assert_eq!(parsed[0].id, TrackId(7));
        assert_eq!(parsed[0].audio, AudioInfo::default());
    }

    /// 坏 JSON 退空曲库而不是 panic —— 启动不该被一个写坏的文件卡住。
    #[test]
    fn 读不懂就当没有缓存() {
        let parsed: Vec<Track> = serde_json::from_str("这不是 JSON").unwrap_or_default();
        assert!(parsed.is_empty());
    }

    /// 临时目录；名字带 pid，避免并行测试撞车。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mp-next-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 整库一个都不在 = 盘没挂载，不能当成文件被删而让下一次落盘把整库覆盖成空。
    #[test]
    fn a_whole_missing_drive_keeps_the_cache() {
        let dir = temp_dir("all-missing");
        let tracks = vec![
            track(&dir.join("gone-a.flac").to_string_lossy()),
            track(&dir.join("gone-b.flac").to_string_lossy()),
        ];

        assert_eq!(
            super::prune_missing(tracks).len(),
            2,
            "全都不在时要原样返回，而不是清空"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 只要还有一个在，就回到逐条核对：其余该被剔掉。
    #[test]
    fn entries_that_are_gone_are_dropped_once_anything_survives() {
        let dir = temp_dir("partial");
        let real = dir.join("here.flac");
        std::fs::write(&real, b"x").unwrap();

        let kept = super::prune_missing(vec![
            track(&real.to_string_lossy()),
            track(&dir.join("gone.flac").to_string_lossy()),
        ]);

        assert_eq!(kept.len(), 1, "有一个还在时就该逐条核对");
        assert_eq!(kept[0].path, real);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn track(path: &str) -> Track {
        Track {
            id: TrackId::from_path(Path::new(path)),
            path: PathBuf::from(path),
            title: "曲名".to_owned(),
            artist: Some("歌手".to_owned()),
            album: None,
            duration: Some(Duration::from_secs(200)),
            has_cover: false,
            audio: AudioInfo::default(),
        }
    }
}
