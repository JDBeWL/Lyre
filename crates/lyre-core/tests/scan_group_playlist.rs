//! `lyre-core` 的端到端集成测试：扫描 → 读标签 → 入队 → 分组 → 排序 / 撤销。
//! 钉的是跨模块契约：扫描出的路径要被 `Playlist::push` 接受，`tracks()` 要能被分组函数消费，
//! 移出再回插要能复原。不需要真实音频，读不出标签的文件会退化成 [`Track::fallback`]。
//!
//! `clippy.toml` 的 `allow-*-in-tests` 不覆盖 `tests/` 下的独立集成测试 crate，故在此显式允许。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use lyre_core::{
    PlayMode, Playlist, SortKey, Track, UNKNOWN_ALBUM, UNKNOWN_ARTIST, collect_audio_paths_with,
    group_by_album, group_by_artist, scan_into_tracks,
};

/// 铺一棵曲库树：`a.mp3`、`B.FLAC`（扩展名大小写不敏感）、`notes.txt`（非音频，须忽略）、`sub/c.wav`。
///
/// `tag` 必须各用例不同：用例在同一进程里并行跑，共用目录会互相删数据。
fn fixture(tag: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("lyre-core-pipeline-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("sub")).unwrap();

    for name in ["a.mp3", "B.FLAC", "notes.txt"] {
        fs::write(root.join(name), b"not really audio").unwrap();
    }
    fs::write(root.join("sub").join("c.wav"), b"not really audio").unwrap();

    root
}

fn file_names(paths: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = paths
        .iter()
        .map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// 扫出的曲目（用 fallback 标题 = 文件名去扩展名）。
fn scanned(root: &Path) -> Vec<Track> {
    let paths = collect_audio_paths_with(&[root.to_path_buf()], true);
    let mut tracks = Vec::new();
    scan_into_tracks(paths, 64, |chunk| tracks.extend(chunk));
    tracks
}

#[test]
fn collection_honours_recursion_extensions_and_dedup() {
    let root = fixture("collect");

    let recursive = collect_audio_paths_with(std::slice::from_ref(&root), true);
    assert_eq!(
        file_names(&recursive),
        ["B.FLAC", "a.mp3", "c.wav"],
        "递归要把子目录收进来，且只收音频扩展名"
    );

    let top_only = collect_audio_paths_with(std::slice::from_ref(&root), false);
    assert_eq!(
        file_names(&top_only),
        ["B.FLAC", "a.mp3"],
        "「仅顶层」不该把直接子目录里的音频也收进来"
    );

    // 同一个文件经由「父目录」和「它自己」各出现一次 —— 结果必须去重
    let overlapping = collect_audio_paths_with(&[root.clone(), root.join("sub")], true);
    assert_eq!(
        file_names(&overlapping),
        ["B.FLAC", "a.mp3", "c.wav"],
        "重叠输入不该让同一首进两次"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scanning_reports_every_file_and_reads_broken_ones_as_fallbacks() {
    let root = fixture("scan");
    let paths = collect_audio_paths_with(std::slice::from_ref(&root), true);

    let mut chunks: Vec<Vec<Track>> = Vec::new();
    let total = scan_into_tracks(paths, 2, |chunk| chunks.push(chunk));

    assert_eq!(total, 3, "三首都要报出来，坏文件不该让整批中断");
    assert_eq!(
        chunks.iter().map(Vec::len).collect::<Vec<_>>(),
        [2, 1],
        "按 chunk=2 分批，最后一批不满"
    );

    // 内容是垃圾，读不出标签 → 退化成 fallback：标题取文件名
    let mut titles: Vec<&str> = chunks
        .iter()
        .flatten()
        .map(|track| track.title.as_str())
        .collect();
    titles.sort_unstable();
    assert_eq!(titles, ["B", "a", "c"]);

    let _ = fs::remove_dir_all(root);
}

/// 全链路：扫描 → 入队 → 分组 → 排序 → 移出 → 撤销回插 → 打乱模式。
#[test]
fn the_scanned_tracks_flow_through_queue_grouping_and_undo() {
    let root = fixture("pipeline");
    let tracks = scanned(&root);
    assert_eq!(tracks.len(), 3);

    let mut playlist = Playlist::new();
    assert_eq!(playlist.push(tracks.clone()), 3, "首次导入三首都算新增");
    assert_eq!(
        playlist.push(tracks.clone()),
        0,
        "同 id 是原地替换标签，不是新增"
    );
    assert_eq!(playlist.len(), 3);
    assert_eq!(playlist.order().len(), 3);

    // 分组直接吃 `tracks()`：都没有专辑 / 艺术家标签，应当各自归一到「未知」那一组
    let albums = group_by_album(playlist.tracks());
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].title, UNKNOWN_ALBUM);
    assert_eq!(albums[0].track_count(), 3);
    assert!(
        albums[0].display_artist().is_empty(),
        "组里没有任何艺术家名"
    );

    let artists = group_by_artist(playlist.tracks());
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0].name, UNKNOWN_ARTIST);
    assert_eq!(artists[0].album_count(), 0, "没标签就没有专辑名可记");

    // 换排序键不该丢曲目，只该换顺序
    playlist.sort_by(SortKey::Title);
    assert_eq!(playlist.sort_key(), SortKey::Title);
    let sorted: Vec<&str> = playlist
        .tracks()
        .iter()
        .map(|track| track.title.as_str())
        .collect();
    assert_eq!(
        sorted,
        ["a", "B", "c"],
        "标题升序且大小写不敏感（`a` < `B` < `c`）；按字节序会排成 B,a,c"
    );
    assert_eq!(playlist.len(), 3, "排序不该改变曲目数");

    // 移出一首再撤销回插：位置与光标都要复原
    let victim = playlist.tracks()[1].clone();
    let victim_index = 1;
    playlist.jump_to(victim.id);
    assert!(playlist.remove(victim.id), "移除应当成功");
    assert_eq!(playlist.len(), 2);
    assert!(!playlist.remove(victim.id), "同一首不能移除两次");

    assert!(playlist.insert_at(victim.clone(), victim_index), "撤销回插");
    assert_eq!(playlist.len(), 3);
    assert!(!playlist.insert_at(victim.clone(), 0), "同 id 不能插两次");
    assert_eq!(
        playlist.by_id(victim.id).map(|t| t.title.as_str()),
        Some(victim.title.as_str())
    );

    // 打乱只是换顺序：曲目集合必须原封不动（这是「随机播放丢歌」那类事故的哨兵）
    playlist.set_mode(PlayMode::Shuffle, 0x1234_5678);
    let shuffled: Vec<String> = playlist
        .tracks()
        .iter()
        .map(|track| track.title.clone())
        .collect();
    assert_eq!(shuffled.len(), 3);
    for title in ["B", "a", "c"] {
        assert!(shuffled.iter().any(|t| t == title), "打乱后丢了 {title}");
    }
    assert_eq!(playlist.order().len(), 3);

    let _ = fs::remove_dir_all(root);
}
