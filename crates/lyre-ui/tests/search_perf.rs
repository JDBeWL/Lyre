//! 搜索与追加收窄的性能基准。默认不跑：10 万首的构建与多轮扫描会拖慢常规测试。
//! 运行：cargo test -p lyre-ui --release --test `search_perf` -- --ignored --nocapture

use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lyre_core::Track;
use lyre_ui::entities::filter::{self, SearchFields};

const TRACKS: usize = 100_000;
const ROUNDS: usize = 20;

fn library() -> Vec<Track> {
    (0..TRACKS)
        .map(|index| {
            let path = PathBuf::from(format!("/music/{index}.flac"));
            let mut track = Track::fallback(&path);
            track.title = if index % 2 == 0 {
                format!("城市夜航 {index}")
            } else {
                format!("Midnight Drive {index}")
            };
            track.artist = Some(if index % 3 == 0 {
                "陈屿".to_owned()
            } else {
                "Kaito".to_owned()
            });
            track.album = Some(if index % 5 == 0 {
                "城市漫游".to_owned()
            } else {
                "Neon".to_owned()
            });
            track
        })
        .collect()
}

fn p95(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[(samples.len() * 95 / 100).min(samples.len() - 1)]
}

#[test]
#[ignore = "性能基准，手动运行"]
fn a_hundred_thousand_tracks_keeps_search_and_typing_latency_bounded() {
    let tracks = library();
    let fields: Vec<SearchFields> = tracks.iter().map(SearchFields::new).collect();
    let full_terms = filter::parse_query("城市 夜");

    // 预热：第一遍包含分配器与页表的冷启动。
    black_box(
        filter::project_in_fields(&fields, &full_terms, None)
            .unwrap()
            .len(),
    );

    let mut full = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let start = Instant::now();
        let rows = filter::project_in_fields(&fields, &full_terms, None).unwrap();
        full.push(start.elapsed());
        black_box(rows.len());
    }

    // 模拟逐字输入：每一步只在上一步的结果里收窄。
    let mut incremental = Vec::with_capacity(ROUNDS * 4);
    for _ in 0..ROUNDS {
        let mut rows =
            filter::project_in_fields(&fields, &filter::parse_query("城"), None).unwrap();
        for query in ["城市", "城市夜", "城市夜航"] {
            let terms = filter::parse_query(query);
            let start = Instant::now();
            rows = filter::narrow_in(&fields, &rows, &terms);
            incremental.push(start.elapsed());
            black_box(rows.len());
        }
    }

    let full_p95 = p95(full);
    let incremental_p95 = p95(incremental);
    println!("10 万首：全库查询 P95 {full_p95:?}，逐字收窄 P95 {incremental_p95:?}");

    assert!(
        full_p95 < Duration::from_millis(500),
        "全库查询 P95 超过 500ms：{full_p95:?}"
    );
    assert!(
        incremental_p95 < Duration::from_millis(200),
        "逐字收窄 P95 超过 200ms：{incremental_p95:?}"
    );
}
