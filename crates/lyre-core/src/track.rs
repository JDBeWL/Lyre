use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 曲目的稳定标识：对路径求 FNV-1a。不用 `DefaultHasher` —— 它跨 Rust 版本不保证一致，落盘读回来会换 id。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct TrackId(pub u64);

impl TrackId {
    pub fn from_path(path: &Path) -> Self {
        // FNV-1a 64 位
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in path.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self(hash)
    }
}

impl std::fmt::Display for TrackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// 解码前的音频规格，来自容器 / 流头而不是标签。
/// 三项全可选：读不到就留 `None`，不拿 0 或 44.1kHz 占位。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioInfo {
    /// 采样率，单位 Hz（44100 / 48000 / 96000…）。
    pub sample_rate: Option<u32>,
    /// 每样本位数。有损编码没有这个概念，恒为 `None`。
    pub bit_depth: Option<u8>,
    /// 总码率，单位 kbps。
    pub bitrate: Option<u32>,
}

/// 底栏音质徽章的档位。判据见 [`Track::quality`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// 有损编码：不亮徽章
    Lossy,
    /// 无损，但采样规格没超过 CD
    Lossless,
    /// 无损且高于 CD
    HiRes,
}

/// 无损编码集合。判据是容器/编码而不是采样规格 ——
/// 一个 16bit/44.1kHz 的 FLAC 依然是无损，而 24bit 的 mp3 依然是有损。
const LOSSLESS_FORMATS: [&str; 5] = ["flac", "wav", "ape", "alac", "aiff"];

/// 高于 CD 的具体门槛：≥ 24bit 且 ≥ 96kHz。
/// 两个条件都要，是因为只提高采样率而仍是 16bit 的话，量化噪声没变，听感上限也没变。
const HIRES_BITS: u8 = 24;
const HIRES_RATE: u32 = 96_000;

/// 一首曲目的元信息。不带封面字节，只记 `has_cover`；字节由 [`crate::metadata::read_cover`] 按需读取。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    pub has_cover: bool,
    #[serde(default)]
    pub audio: AudioInfo,
}

impl Track {
    /// 标签读取失败时的兜底：文件名当标题，其余留空 —— 不因一个坏 ID3 标签丢掉整个文件。
    pub fn fallback(path: &Path) -> Self {
        let title = path.file_stem().map_or_else(
            || path.to_string_lossy().into_owned(),
            |s| s.to_string_lossy().into_owned(),
        );

        Self {
            id: TrackId::from_path(path),
            path: path.to_path_buf(),
            title,
            artist: None,
            album: None,
            duration: None,
            has_cover: false,
            audio: AudioInfo::default(),
        }
    }

    pub fn is_unknown_title(&self) -> bool {
        self.title.is_empty()
    }

    /// 列表副标题：优先「艺术家 · 专辑」，缺失时退化到父目录名。
    pub fn subtitle(&self) -> String {
        match (self.artist.as_deref(), self.album.as_deref()) {
            (Some(a), Some(b)) => format!("{a} · {b}"),
            (Some(a), None) => a.to_string(),
            (None, Some(b)) => b.to_string(),
            (None, None) => self
                .path
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }

    /// 小写扩展名，用于格式小标签；无扩展名返回 `None`（宁可不显示，不放看不懂的占位符）。
    pub fn format(&self) -> Option<String> {
        let ext = self.path.extension()?.to_str()?;
        if ext.is_empty() {
            return None;
        }
        Some(ext.to_ascii_lowercase())
    }

    /// 是否无损编码。判据是容器而不是采样规格，见 `LOSSLESS_FORMATS`。
    pub fn is_lossless(&self) -> bool {
        self.format()
            .is_some_and(|f| LOSSLESS_FORMATS.contains(&f.as_str()))
    }

    /// 底栏徽章该写哪一档。读不到位深/采样率时最多只报 [`Quality::Lossless`] ——
    /// Hi-Res 是要负责的字，必须有真数据撑着。
    pub fn quality(&self) -> Quality {
        if !self.is_lossless() {
            return Quality::Lossy;
        }
        match (self.audio.bit_depth, self.audio.sample_rate) {
            (Some(bits), Some(rate)) if bits >= HIRES_BITS && rate >= HIRES_RATE => Quality::HiRes,
            _ => Quality::Lossless,
        }
    }
}

impl AudioInfo {
    /// 三项里有没有任何一项 —— 决定徽章点开有没有东西可看。
    pub fn is_empty(&self) -> bool {
        self.sample_rate.is_none() && self.bit_depth.is_none() && self.bitrate.is_none()
    }

    /// 详情浮层里的「标签 / 值」行，只收有值的项。
    pub fn details(&self) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        if let Some(rate) = self.sample_rate {
            rows.push(("采样率", format!("{} kHz", kilo(rate))));
        }
        if let Some(bits) = self.bit_depth {
            rows.push(("位深", format!("{bits}-bit")));
        }
        if let Some(bitrate) = self.bitrate {
            rows.push(("码率", format!("{bitrate} kbps")));
        }
        rows
    }
}

/// Hz → kHz 的显示值：44100 要读成 44.1 而不是 44.10，96000 要读成 96 而不是 96.00。
fn kilo(hz: u32) -> String {
    format!("{:.2}", hz as f32 / 1000.0)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

/// 把时长格式化成 `m:ss` 或 `h:mm:ss`。
pub fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_id_is_stable_for_same_path() {
        let p = Path::new("/music/夜航西飞.flac");
        assert_eq!(TrackId::from_path(p), TrackId::from_path(p));
    }

    #[test]
    fn track_id_differs_across_paths() {
        assert_ne!(
            TrackId::from_path(Path::new("/music/a.flac")),
            TrackId::from_path(Path::new("/music/b.flac"))
        );
    }

    #[test]
    fn fallback_uses_file_stem() {
        let t = Track::fallback(Path::new("/music/纸飞机与旧磁带.mp3"));
        assert_eq!(t.title, "纸飞机与旧磁带");
        assert!(!t.has_cover);
    }

    #[test]
    fn subtitle_degrades_gracefully() {
        let mut t = Track::fallback(Path::new("/music/album/x.mp3"));
        assert_eq!(t.subtitle(), "album");

        t.artist = Some("陈屿".into());
        assert_eq!(t.subtitle(), "陈屿");

        t.album = Some("城市漫游".into());
        assert_eq!(t.subtitle(), "陈屿 · 城市漫游");
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(Duration::from_secs(227)), "3:47");
        assert_eq!(format_duration(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn format_is_lowercased_extension() {
        assert_eq!(
            Track::fallback(Path::new("/music/x.MP3"))
                .format()
                .as_deref(),
            Some("mp3")
        );
        assert_eq!(
            Track::fallback(Path::new("/music/x.Flac"))
                .format()
                .as_deref(),
            Some("flac")
        );
    }

    #[test]
    fn format_is_none_without_an_extension() {
        assert_eq!(Track::fallback(Path::new("/music/无扩展名")).format(), None);
    }

    fn with_audio(path: &str, rate: Option<u32>, bits: Option<u8>, bitrate: Option<u32>) -> Track {
        let mut t = Track::fallback(Path::new(path));
        t.audio = AudioInfo {
            sample_rate: rate,
            bit_depth: bits,
            bitrate,
        };
        t
    }

    #[test]
    fn 无损判定看编码不看采样规格() {
        for f in ["flac", "wav", "ape", "alac", "aiff"] {
            assert!(
                Track::fallback(Path::new(&format!("/m/x.{f}"))).is_lossless(),
                "{f} 是无损编码"
            );
        }
        for f in ["mp3", "m4a", "aac", "ogg", "opus"] {
            assert!(
                !Track::fallback(Path::new(&format!("/m/x.{f}"))).is_lossless(),
                "{f} 是有损编码"
            );
        }
        assert!(!Track::fallback(Path::new("/m/无扩展名")).is_lossless());
    }

    #[test]
    fn 采样规格够高的无损才算高解析() {
        let cd = with_audio("a.flac", Some(44100), Some(16), Some(900));
        assert_eq!(
            cd.quality(),
            Quality::Lossless,
            "CD 规格的 FLAC 不配 Hi-Res"
        );
        let hires = with_audio("a.flac", Some(96000), Some(24), Some(2834));
        assert_eq!(hires.quality(), Quality::HiRes);
        // 两个条件缺一不可：只抬采样率而仍是 16bit，量化噪声没变
        assert_eq!(
            with_audio("a.flac", Some(192_000), Some(16), None).quality(),
            Quality::Lossless
        );
        assert_eq!(
            with_audio("a.flac", Some(44100), Some(32), None).quality(),
            Quality::Lossless
        );
        // 有损编码哪怕标着 24bit/96kHz 也不该亮徽章
        assert_eq!(
            with_audio("a.mp3", Some(96000), Some(24), Some(320)).quality(),
            Quality::Lossy
        );
        // 读不到规格时无损仍是无损，只是不敢报 Hi-Res
        assert_eq!(
            with_audio("a.wav", None, None, None).quality(),
            Quality::Lossless
        );
    }

    #[test]
    fn 采样率显示去掉无意义的零() {
        assert_eq!(kilo(96000), "96");
        assert_eq!(kilo(44100), "44.1");
        assert_eq!(kilo(48000), "48");
        assert_eq!(kilo(22050), "22.05");
        assert_eq!(kilo(8000), "8");
    }

    #[test]
    fn 详情只列有值的项且顺序稳定() {
        let full = AudioInfo {
            sample_rate: Some(96000),
            bit_depth: Some(24),
            bitrate: Some(2834),
        };
        assert_eq!(
            full.details(),
            vec![
                ("采样率", "96 kHz".to_owned()),
                ("位深", "24-bit".to_owned()),
                ("码率", "2834 kbps".to_owned()),
            ]
        );

        // 有损编码没有位深，那一行就不该出现
        let lossy = AudioInfo {
            sample_rate: Some(44100),
            bit_depth: None,
            bitrate: Some(320),
        };
        assert_eq!(
            lossy.details(),
            vec![
                ("采样率", "44.1 kHz".to_owned()),
                ("码率", "320 kbps".to_owned()),
            ]
        );

        assert!(AudioInfo::default().is_empty());
        assert!(AudioInfo::default().details().is_empty());
    }
}
