use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::metadata::read_track;
use crate::track::Track;

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "wave", "ogg", "oga", "opus", "m4a", "m4b", "aac", "mp4", "aiff", "aif",
    "alac", "wma", "mka", "ape", "wv",
];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            AUDIO_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext))
        })
}

/// 折成绝对且词法规范的路径。必须做在入库之前：`TrackId` 是路径哈希，相对路径落盘之后
/// 换个目录启动会把整个曲库当成失效剔掉。规范失败退回原值。
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 把选中的路径摊平成音频文件列表：目录递归展开，结果排序去重，扫描顺序稳定。
pub fn collect_audio_paths(inputs: &[PathBuf]) -> Vec<PathBuf> {
    collect_audio_paths_with(inputs, true)
}

/// 同上，可选不递归 —— 导入「待整理」目录时不希望把子目录一起收进来。
pub fn collect_audio_paths_with(inputs: &[PathBuf], recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // `WalkDir` 的深度是 0 基：0 = 输入目录自己（会被下面的 `is_file()` 挡掉），
    // 1 = 它的直接子项。所以「仅顶层」是 `max_depth(1)`。
    let depth = if recursive { usize::MAX } else { 1 };

    for input in inputs {
        // 目录分支里 WalkDir 的产出全部以输入为前缀，所以规范一次输入就够了
        let input = &absolute(input);
        if input.is_dir() {
            for entry in WalkDir::new(input)
                .follow_links(false)
                .max_depth(depth)
                .into_iter()
                .filter_map(Result::ok)
            {
                if entry.file_type().is_file() && is_audio_file(entry.path()) {
                    out.push(entry.into_path());
                }
            }
        } else if input.is_file() && is_audio_file(input) {
            out.push(input.clone());
        }
    }

    out.sort();
    out.dedup();
    out
}

/// 逐首读元数据、按 `chunk` 分批回调，让 UI 先显示前一批。阻塞函数，调用方负责放到后台线程。
/// 单首失败退化成 [`Track::fallback`]，不中断整批。
pub fn scan_into_tracks<F>(paths: Vec<PathBuf>, chunk: usize, mut on_chunk: F) -> usize
where
    F: FnMut(Vec<Track>),
{
    let chunk = chunk.max(1);
    let mut buffer = Vec::with_capacity(chunk);
    let mut total = 0;

    for path in paths {
        let track = read_track(&path).unwrap_or_else(|_| Track::fallback(&path));
        buffer.push(track);
        total += 1;

        if buffer.len() >= chunk {
            on_chunk(std::mem::take(&mut buffer));
            buffer.reserve(chunk);
        }
    }

    if !buffer.is_empty() {
        on_chunk(buffer);
    }

    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_common_containers() {
        assert!(is_audio_file(Path::new("/a/b/song.MP3")));
        assert!(is_audio_file(Path::new("/a/b/song.flac")));
        assert!(is_audio_file(Path::new("/a/b/song.m4a")));
        assert!(!is_audio_file(Path::new("/a/b/cover.jpg")));
        assert!(!is_audio_file(Path::new("/a/b/noextension")));
    }

    #[test]
    fn scan_chunks_are_complete_and_counted() {
        let paths: Vec<PathBuf> = (0..7)
            .map(|i| PathBuf::from(format!("/nonexistent/{i}.flac")))
            .collect();

        let mut chunks: Vec<usize> = Vec::new();
        let total = scan_into_tracks(paths, 3, |c| chunks.push(c.len()));

        assert_eq!(total, 7);
        assert_eq!(chunks, vec![3, 3, 1], "分批应当覆盖全部且不重不漏");
    }

    #[test]
    fn missing_files_fall_back_instead_of_failing() {
        let paths = vec![PathBuf::from("/nonexistent/夜航西飞.flac")];
        let mut titles = Vec::new();
        scan_into_tracks(paths, 10, |c| titles.extend(c.into_iter().map(|t| t.title)));
        assert_eq!(titles, vec!["夜航西飞"], "读不出标签时应当用文件名兜底");
    }

    #[test]
    fn collect_ignores_missing_paths() {
        assert!(collect_audio_paths(&[PathBuf::from("/definitely/not/here")]).is_empty());
    }

    /// 曲库现在会落盘，而 `TrackId` 是路径哈希：相对或带绕路的写法会让同一个文件
    /// 在两次导入里变成两条，也会让下次启动从别的 cwd 把整个曲库误判成已删除。
    #[test]
    fn collected_paths_are_absolute_and_normalised() {
        let dir = std::env::temp_dir().join(format!("mp-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let file = dir.join("song.flac");
        std::fs::write(&file, b"").expect("建临时文件");

        let detour = dir.join(".").join("sub").join("..").join("song.flac");
        assert_ne!(
            detour, file,
            "绕路写法在字面上必须确实不同，否则这条测试什么都没测"
        );

        let found = collect_audio_paths(&[detour, file.clone()]);
        assert_eq!(
            found,
            vec![file],
            "绕路写法应与直接路径折成同一条：{found:?}"
        );
        assert!(found[0].is_absolute());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn relative_inputs_expand_against_the_cwd() {
        let abs = absolute(Path::new("song.flac"));
        assert!(abs.is_absolute(), "相对路径必须被展开：{abs:?}");
        assert_eq!(abs.file_name().unwrap(), "song.flac");
        assert!(!abs.to_string_lossy().contains(".."));
    }
}
