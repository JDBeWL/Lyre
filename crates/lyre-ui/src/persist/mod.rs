//! JSON 持久化层：找数据目录 → 读文件 → 读不懂退默认 → 建目录 → 写临时文件 → rename 覆盖。
//! 数据目录只读、文件被外部改坏都只记日志，不阻塞启动。

mod error;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

pub use error::PersistError;

const APP_DIR: &str = "Lyre";

/// 各平台用户数据目录；环境变量缺失时返回 `None`。
pub fn data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
    }
    .map(|dir| dir.join(APP_DIR))
}

/// 先写同名 `.tmp` 再 `rename` 覆盖。`fs::write` 是原地截断，写到一半崩溃会留下半份 JSON，
/// 而加载器把读不懂的文件当空状态，下一次落盘就把真数据覆盖掉。
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let tmp = path.with_file_name(format!("{}.tmp", name.to_string_lossy()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

/// 数据目录下的 `file`；没有可用数据目录时返回 `None`。
fn path_of(file: &str) -> Option<PathBuf> {
    data_dir().map(|dir| dir.join(file))
}

/// 读 JSON。`Ok(None)` 专指「文件不存在」；其余 IO / 解析失败以 `Err` 返回。
fn load<T: DeserializeOwned>(file: &str) -> Result<Option<T>, PersistError> {
    let Some(path) = path_of(file) else {
        return Err(PersistError::NoDataDir);
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(PersistError::Io { path, source }),
    };
    Ok(Some(serde_json::from_str(&text)?))
}

/// 读 JSON；任何失败都退回 `Default` 并记日志 —— 刻意的吞错。
pub fn load_or_default<T: DeserializeOwned + Default>(file: &str) -> T {
    match load::<T>(file) {
        Ok(Some(value)) => value,
        Ok(None) => T::default(),
        Err(err) => {
            log::warn!("读取 {file} 失败，退回默认值：{err}");
            T::default()
        }
    }
}

/// 读 JSON，失败一律当「没有」：「没有」本身是合法状态（如上次播放位置）。
pub fn load_or_none<T: DeserializeOwned>(file: &str) -> Option<T> {
    match load::<T>(file) {
        Ok(value) => value,
        Err(err) => {
            log::warn!("读取 {file} 失败，当作没有：{err}");
            None
        }
    }
}

/// 写 JSON，带缩进；条目可能上千的 `library.json` 走 [`save_compact_lossy`]。
pub fn save_lossy<T: Serialize>(file: &str, value: &T) {
    log_if_failed(file, save(file, value, render_pretty));
}

/// 写 JSON，紧凑（无缩进）。给 `library.json`：整库条目可能上千，缩进会让文件涨好几倍。
pub fn save_compact_lossy<T: Serialize>(file: &str, value: &T) {
    log_if_failed(file, save(file, value, render_compact));
}

fn render_pretty<T: Serialize>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string_pretty(value)
}

fn render_compact<T: Serialize>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string(value)
}

/// 删除一个文件（不存在视为成功）。失败只记日志。
pub fn remove_lossy(file: &str) {
    let Some(path) = path_of(file) else {
        return;
    };
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => log::warn!("删除 {file} 失败：{err}"),
    }
}

/// 写盘失败只记日志：存不下不该影响主流程（播放还得继续）。
fn log_if_failed(file: &str, result: Result<(), PersistError>) {
    if let Err(err) = result {
        log::warn!("写入 {file} 失败（本次不持久化）：{err}");
    }
}

fn save<T: Serialize>(
    file: &str,
    value: &T,
    render: impl FnOnce(&T) -> serde_json::Result<String>,
) -> Result<(), PersistError> {
    let Some(path) = path_of(file) else {
        return Err(PersistError::NoDataDir);
    };
    if let Some(parent) = path.parent()
        && let Err(source) = fs::create_dir_all(parent)
    {
        return Err(PersistError::Io { path, source });
    }
    let text = render(value)?;
    write_atomic(&path, &text).map_err(|source| PersistError::Io { path, source })
}

#[cfg(test)]
mod tests {
    use super::{PersistError, load_or_default, render_compact, render_pretty, write_atomic};
    use std::path::Path;

    /// Windows 的 `rename` 必须能覆盖已存在的目标，否则第二次保存直接写不进去。
    #[test]
    fn write_atomic_replaces_existing_and_leaves_no_temp() {
        let dir = temp_dir("atomic");
        let path = dir.join("state.json");

        write_atomic(&path, "{\"a\":1}").unwrap();
        write_atomic(&path, "{\"a\":2}").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":2}");
        assert!(
            !dir.join("state.json.tmp").exists(),
            "临时文件没被 rename 收走"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 两个渲染器不能接反：小文件靠缩进可 diff，`library.json` 靠紧凑控制体积。
    #[test]
    fn pretty_is_indented_and_compact_is_not() {
        let value = vec![1, 2];
        assert_eq!(
            render_pretty(&value).unwrap(),
            "[\n  1,\n  2\n]",
            "pretty 要带缩进"
        );
        assert_eq!(
            render_compact(&value).unwrap(),
            "[1,2]",
            "compact 不该有空白"
        );
    }

    /// 无文件名的路径要在写之前拒绝，否则会写出一个叫 `.tmp` 的文件。
    #[test]
    fn write_atomic_rejects_a_nameless_path() {
        assert!(write_atomic(Path::new(""), "x").is_err());
    }

    /// 坏内容要能被识别成 JSON 错误，而不是被当成 IO 故障 —— 两者的处置方式不同。
    #[test]
    fn corrupt_content_is_reported_as_a_json_error() {
        let parsed: Result<Vec<u32>, _> = serde_json::from_str("这不是 JSON");
        let err = PersistError::from(parsed.unwrap_err());

        assert!(matches!(err, PersistError::Json(_)), "应为 Json：{err}");
    }

    /// 读一个不存在的文件必须静默退默认值而不是 panic：首次启动走的就是这条路径。
    #[test]
    fn a_missing_file_falls_back_to_default() {
        let value: Vec<u32> = load_or_default("mp-next-probe-does-not-exist.json");
        assert!(value.is_empty());
    }

    /// 七份 store 的载荷形状对照表：顶层是不是裸数组、`TrackId` 是不是裸数字、`Collections` 有没有 `next`。
    /// 形状一变，老用户手上的文件就读不回来了。
    #[test]
    fn seven_store_payload_shapes_stay_unchanged() {
        use crate::resume::Resume;
        use crate::settings::Settings;
        use lyre_core::{CollectionId, Collections, TrackId};
        use std::path::Path;

        // settings.json：对象，每个设置项一个字段。
        let settings = render_pretty(&Settings::default()).unwrap();
        assert!(settings.starts_with("{\n  \"density\""), "{settings}");
        assert!(settings.contains("\"default_view\""), "{settings}");

        // likes.json / recents.json：裸数字数组（`TrackId` 是单字段元组结构体）。
        let ids = vec![TrackId(1), TrackId(999)];
        assert_eq!(render_pretty(&ids).unwrap(), "[\n  1,\n  999\n]");
        assert_eq!(render_compact(&ids).unwrap(), "[1,999]");

        // follows.json：JSON 字符串数组。
        let names = vec!["A".to_owned(), "B".to_owned()];
        assert_eq!(render_pretty(&names).unwrap(), "[\n  \"A\",\n  \"B\"\n]");

        // resume.json：对象（路径 + 毫秒），不是裸数组。
        let resume = Resume::new(Path::new("D:/音乐/a.flac"), 83_000);
        let text = render_pretty(&resume).unwrap();
        assert!(
            text.contains("\"path\"") && text.contains("\"position_ms\""),
            "{text}"
        );

        // collections.json：对象，必须带 `next` —— 丢了它，重启后新歌单会顶用老 id。
        let mut collections = Collections::default();
        let first = collections.create("深夜写歌").expect("建一个").id;
        assert_eq!(first, CollectionId(0));
        assert!(render_pretty(&collections).unwrap().contains("\"next\": 1"));
        assert!(render_compact(&collections).unwrap().contains("\"next\":1"));

        // library.json：裸 `Vec<Track>` 数组，且是紧凑排版（条目可能上千）。
        let tracks: Vec<lyre_core::Track> = Vec::new();
        let text = render_compact(&tracks).unwrap();
        assert_eq!(text, "[]");
        assert!(
            !render_compact(&ids).unwrap().contains('\n'),
            "紧凑排版不该有换行"
        );
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mp-next-persist-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
