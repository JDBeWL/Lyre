//! `group_by_album` / `group_by_artist` 的基准：`lyre-core` 里唯一随曲库规模变化的热路径，
//! 每次曲库变化都要对全库跑一遍。其余模块要么 O(1) 查表，要么只在单首曲目上工作。

use std::hint::black_box;
use std::path::Path;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use lyre_core::{AudioInfo, Track, TrackId, group_by_album, group_by_artist};

/// 铺一个像真曲库的集合：`albums` 张专辑 × `per_album` 首，艺术家按 97% 复用。
fn synthetic_library(albums: usize, per_album: usize) -> Vec<Track> {
    let mut tracks = Vec::with_capacity(albums * per_album);
    for album in 0..albums {
        for n in 0..per_album {
            let path = format!("/music/{album:04}/{n:02}.flac");
            tracks.push(Track {
                id: TrackId::from_path(Path::new(&path)),
                path: path.into(),
                title: format!("曲目 {n}"),
                artist: Some(format!("艺术家 {}", album % 97)),
                album: Some(format!("专辑 {album:04}")),
                duration: Some(Duration::from_secs(180 + n as u64 % 120)),
                // 每三首带一张封面：让「选封面代表」那条分支真的被走到
                has_cover: n % 3 == 0,
                audio: AudioInfo::default(),
            });
        }
    }
    tracks
}

fn grouping(c: &mut Criterion) {
    // 2000 首 ≈ 一个中等规模曲库。分组在这个量级上必须是毫秒级；
    // 明显变慢就说明累加路径上多了不该有的分配。
    let tracks = synthetic_library(200, 10);

    let mut group = c.benchmark_group("group");
    group.throughput(Throughput::Elements(tracks.len() as u64));
    group.bench_function("by_album", |b| {
        b.iter(|| group_by_album(black_box(&tracks)));
    });
    group.bench_function("by_artist", |b| {
        b.iter(|| group_by_artist(black_box(&tracks)));
    });
    group.finish();
}

criterion_group!(benches, grouping);
criterion_main!(benches);
