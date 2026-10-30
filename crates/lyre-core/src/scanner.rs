use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::metadata::read_track;
use crate::track::Track;

/// 扫得到的音频扩展名。进这一份只代表「这是个音频文件」，能不能播见 [`UNSUPPORTED_EXTENSIONS`]。
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "wave", "ogg", "oga", "opus", "m4a", "m4b", "aac", "mp4", "aiff", "aif",
    "alac", "wma", "mka", "ape", "wv",
];

/// 引擎解不出声的扩展名：标签、时长、采样率都读得到，但没有解码器，入库后点播放只会报错。
/// 只剩 wma；APE / `WavPack` / Opus 已有专门解码器（`lyre-audio` 的 `decode` 模块）。
pub const UNSUPPORTED_EXTENSIONS: &[&str] = &["wma"];

/// 判魔数要读的字节数。最长签名是 Ogg 偏移 28 处的 `OpusHead`，要装得下就得 ≥ 36；
/// 32 会把它截成 4 个字节的前缀，永远匹配不上。
pub const SIGNATURE_PROBE_LEN: usize = 40;

/// Ogg 首页里数据包起始的偏移：27 字节的页头 + 1 字节的段表。
const OGG_FIRST_PACKET_OFFSET: usize = 28;

/// ASF（WMA 的容器）的文件头 GUID。
const ASF_HEADER_GUID: [u8; 16] = [
    0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
];

/// 文件头能认出来的格式。扩展名可以随便改，魔数不会，所以它是路由与可播性判定的共同依据：
/// `lyre-audio` 拿它决定交给哪个解码器，认出来的都不归通用引擎。
/// `head` 是文件开头至多 [`SIGNATURE_PROBE_LEN`] 字节，短于这个长度也能判。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signature {
    Ape,
    WavPack,
    Opus,
    /// ASF 容器，WMA 各版本都在里面。
    Wma,
}

impl Signature {
    /// 面向用户的格式名，用于错误文案。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ape => "APE",
            Self::WavPack => "WavPack",
            Self::Opus => "Opus",
            Self::Wma => "WMA / ASF",
        }
    }
}

pub fn signature_of(head: &[u8]) -> Option<Signature> {
    if head.starts_with(b"MAC ") {
        return Some(Signature::Ape);
    }
    if head.starts_with(b"wvpk") {
        return Some(Signature::WavPack);
    }
    if head.starts_with(&ASF_HEADER_GUID) {
        return Some(Signature::Wma);
    }
    // 容器是 Ogg 且包体是 OpusHead 才判 Opus：单看 `OpusHead` 会误伤 Ogg 里的 Vorbis / FLAC。
    let opus_packet = head
        .get(OGG_FIRST_PACKET_OFFSET..)
        .is_some_and(|packet| packet.starts_with(b"OpusHead"));
    if head.starts_with(b"OggS") && opus_packet {
        return Some(Signature::Opus);
    }
    None
}

fn has_extension(ext: &str, list: &[&str]) -> bool {
    list.iter().any(|known| known.eq_ignore_ascii_case(ext))
}

/// 扩展名是不是音频文件（不看引擎放不放得出来）。
pub fn is_audio_extension(ext: &str) -> bool {
    has_extension(ext, AUDIO_EXTENSIONS)
}

/// 扩展名对应的编码引擎解不解得出来。不认识的扩展名按「能解」算 ——
/// 能进曲库的都已经被 [`AUDIO_EXTENSIONS`] 筛过一遍，这里只负责挑出已知解不了的那几个。
pub fn is_decodable_extension(ext: &str) -> bool {
    !has_extension(ext, UNSUPPORTED_EXTENSIONS)
}

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_audio_extension)
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

/// 可选不递归：导入「待整理」目录时只收直接子项，不带子目录。
pub fn collect_audio_paths_with(inputs: &[PathBuf], recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // `WalkDir` 的深度是 0 基：0 = 输入目录自己（被下面的 `is_file()` 挡掉），1 = 它的直接子项
    let depth = if recursive { usize::MAX } else { 1 };

    for input in inputs {
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

/// 逐首读元数据、按 `chunk` 分批回调，让 UI 先显示前一批；阻塞函数，调用方负责放到后台线程。
/// 单首失败退化成 [`Track::fallback`]，不中断整批；串行是刻意的，机械硬盘并发读会抢磁头。
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

    /// 解不了的格式仍然是音频文件：要能进库、出现在列表里（置灰），只是不能播。
    #[test]
    fn unsupported_formats_are_still_audio_files() {
        for ext in UNSUPPORTED_EXTENSIONS {
            let path = format!("/a/b/song.{ext}");
            assert!(is_audio_file(Path::new(&path)), "{ext} 应当仍能入库");
            assert!(!is_decodable_extension(ext), "{ext} 不该被判成能播");
        }
    }

    #[test]
    fn every_unsupported_extension_is_one_we_say_we_recognise() {
        let strays: Vec<&&str> = UNSUPPORTED_EXTENSIONS
            .iter()
            .filter(|ext| !is_audio_extension(ext))
            .collect();
        assert!(
            strays.is_empty(),
            "这些扩展名既不在 AUDIO_EXTENSIONS 里，写进 UNSUPPORTED_EXTENSIONS 就没有意义：{strays:?}"
        );
    }

    #[test]
    fn the_unsupported_set_matches_what_the_engine_lacks() {
        assert_eq!(
            UNSUPPORTED_EXTENSIONS,
            &["wma"],
            "改这张表之前先确认解码链路（见 UNSUPPORTED_EXTENSIONS 的文档）。\
             新增解码器后要同时把它从这里去掉"
        );
    }

    #[test]
    fn extension_lookup_ignores_case() {
        assert!(is_audio_extension("FLAC"));
        assert!(is_audio_extension("Wma"));
        assert!(!is_decodable_extension("WMA"), "WMA 仍是唯一解不了的");
        assert!(is_decodable_extension("APE"));
        assert!(is_decodable_extension("Wv"));
        assert!(is_decodable_extension("Opus"));
        assert!(is_decodable_extension("Mp3"));
        assert!(is_decodable_extension("mka"), "Matroska 有格式解析器");
        assert!(is_decodable_extension("alac"), "ALAC 有解码器");
        assert!(
            is_decodable_extension("aac"),
            "ADTS 有格式解析器 + AAC 解码器"
        );
    }

    /// 魔数表要认全四类，少认一类就等于放过改了名的文件 ——
    /// 它们会被通用引擎的探测当成能播，结果不是报错，是噪声。
    #[test]
    fn the_signature_table_covers_every_format_we_special_case() {
        let mut ogg_opus = b"OggS".to_vec();
        ogg_opus.resize(OGG_FIRST_PACKET_OFFSET, 0);
        ogg_opus.extend_from_slice(b"OpusHead");

        for (head, want) in [
            (b"MAC \x0a\x0b".to_vec(), Signature::Ape),
            (b"wvpk\x04\x05".to_vec(), Signature::WavPack),
            (ASF_HEADER_GUID.to_vec(), Signature::Wma),
            (ogg_opus, Signature::Opus),
        ] {
            let read = &head[..head.len().min(SIGNATURE_PROBE_LEN)];
            assert_eq!(
                signature_of(read),
                Some(want),
                "这份文件头应当被认成 {want:?}"
            );
        }
    }

    #[test]
    fn every_signature_has_a_label() {
        for (signature, want) in [
            (Signature::Ape, "APE"),
            (Signature::WavPack, "WavPack"),
            (Signature::Opus, "Opus"),
            (Signature::Wma, "WMA / ASF"),
        ] {
            assert_eq!(signature.label(), want);
        }
    }

    #[test]
    fn the_probe_length_covers_the_longest_signature() {
        assert!(
            SIGNATURE_PROBE_LEN >= OGG_FIRST_PACKET_OFFSET + b"OpusHead".len(),
            "读进来的文件头要装得下偏移 {OGG_FIRST_PACKET_OFFSET} 处的 OpusHead"
        );
    }

    #[test]
    fn normal_headers_are_never_recognised() {
        let mut ogg_vorbis = b"OggS".to_vec();
        ogg_vorbis.resize(OGG_FIRST_PACKET_OFFSET, 0);
        ogg_vorbis.push(0x01); // 段表：包长
        ogg_vorbis.extend_from_slice(b"vorbis");

        for head in [
            b"RIFF\x24\x08\x00\x00WAVEfmt ".to_vec(),
            b"fLaC\x00\x00\x00\x22".to_vec(),
            b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec(),
            b"FORM\x00\x00\x08\x00AIFF".to_vec(),
            ogg_vorbis,
        ] {
            assert!(
                signature_of(&head).is_none(),
                "这份文件头该交给通用引擎：{head:?}"
            );
        }
    }

    /// 只有 `OpusHead` 不够，必须同时是 Ogg 容器；短头与空头都不该 panic。
    #[test]
    fn opus_needs_both_the_ogg_container_and_the_packet() {
        let mut lone = b"XXXX".to_vec();
        lone.resize(OGG_FIRST_PACKET_OFFSET, 0);
        lone.extend_from_slice(b"OpusHead");
        assert!(
            signature_of(&lone).is_none(),
            "不是 Ogg 容器就不该按 Opus 认"
        );

        assert!(signature_of(b"OggS").is_none(), "太短不该 panic");
        assert!(signature_of(b"").is_none(), "空文件头不该 panic");
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
