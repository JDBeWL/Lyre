//! 生成带标签的测试音频看真实数据下的排版：极短真 WAV（100ms 静音）+ `ID3v2` 标签。
//! RIFF INFO 的 ASCII 限制会吞掉中文标题，所以用 `ID3v2`。一次性开发脚本，刻意允许 unwrap。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Write;
use std::path::Path;

/// 采样率，够小就行 —— 这些文件不会被真正播放。
const SAMPLE_RATE: u32 = 8000;
/// 时长（毫秒），只要短到让文件是个合法 WAV。
const DURATION_MS: u32 = 100;

struct Album {
    title: &'static str,
    artist: &'static str,
    tracks: &'static [&'static str],
}

/// 故意包含长名字、多艺术家专辑、同名专辑 —— 布局的边界情况全在这些地方。
const LIBRARY: &[Album] = &[
    Album {
        title: "城市漫游",
        artist: "林一",
        tracks: &["午后的地铁", "天台上的风", "回家的路"],
    },
    Album {
        title: "事切れて、またね。",
        artist: "kyiku",
        tracks: &["事切れて、またね。", "余白", "透明な午後"],
    },
    Album {
        title: "一个特别特别长的专辑名字用来测试侧栏的截断行为",
        artist: "某个名字也很长的艺术家组合",
        tracks: &["第一首", "第二首"],
    },
    Album {
        title: "Night Drive",
        artist: "Various",
        tracks: &["Neon", "Highway", "Dawn"],
    },
    Album {
        title: "古典精选",
        artist: "群星",
        tracks: &[
            "Nocturne No.2",
            "Clair de Lune",
            "Gymnopédie No.1",
            "Arabesque",
        ],
    },
    Album {
        title: "Lo-Fi Beats",
        artist: "Chill Studio",
        tracks: &["Study Loop", "Rainy Window", "Coffee Break"],
    },
    Album {
        title: "现场录音 2024",
        artist: "林一",
        tracks: &["Intro", "Encore"],
    },
    Album {
        title: "节日专辑",
        artist: "群星",
        tracks: &["春天的芭蕾", "夏天的风", "秋天的信", "冬天的雪"],
    },
];

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "seed-library".to_owned());
    let root = Path::new(&out);
    fs::create_dir_all(root).expect("create output dir");

    let mut count = 0;
    for album in LIBRARY {
        for (ix, track) in album.tracks.iter().enumerate() {
            let name = format!("{:02} {}.wav", ix + 1, track);
            let path = root.join(album.title).join(&name);
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).expect("create album dir");
            }
            fs::write(&path, wav_bytes(DURATION_MS)).expect("write wav");
            tag(&path, album, track);
            count += 1;
        }
    }

    println!("wrote {count} tracks into {}", root.display());
}

/// 合法的 16-bit 单声道 PCM WAV，内容全是静音。
fn wav_bytes(ms: u32) -> Vec<u8> {
    let samples = SAMPLE_RATE * ms / 1000;
    let data_len = samples * 2; // 16-bit
    let byte_rate = SAMPLE_RATE * 2;

    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk 长度
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    buf.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    buf.write_all(&vec![0u8; data_len as usize]).unwrap();
    buf
}

/// 往 WAV 里写 `ID3v2` 标签。
fn tag(path: &Path, album: &Album, track: &str) {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::prelude::Accessor;
    use lofty::probe::Probe;
    use lofty::tag::{Tag, TagType};

    let mut tagged = Probe::open(path).expect("open").read().expect("read");
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(track.to_owned());
    tag.set_artist(album.artist.to_owned());
    tag.set_album(album.title.to_owned());
    tag.set_track(1);
    tagged.insert_tag(tag);
    tagged
        .save_to_path(path, WriteOptions::default())
        .expect("save tags");
}
