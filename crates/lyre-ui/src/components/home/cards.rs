//! 首页的数据派生：把曲目 / 专辑分组折成卡片，这一层不碰 gpui。

use lyre_core::{AlbumGroup, Track, TrackId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentCard {
    /// 代表曲目：用它的封面，也是点击时的播放目标。
    pub id: TrackId,
    pub album: String,
    pub artist: String,
    /// 「12 首」这类说明 —— 这里没有完整分组，只报这次队列里的曲目数。
    pub meta: String,
}

/// 按专辑名去重（保留最近那次），顺序由近及远。
/// 前提：`tracks` 已由近及远排好（`recent_tracks` 即此顺序），乱序会弄错顺序与代表曲目。
pub fn recent_cards(tracks: &[Track], limit: usize) -> Vec<RecentCard> {
    let mut out: Vec<RecentCard> = Vec::new();

    for track in tracks {
        let album = track
            .album
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| lyre_core::UNKNOWN_ALBUM.to_string());

        if let Some(existing) = out.iter_mut().find(|card| card.album == album) {
            // 同一张专辑又出现：累加曲目数并把这张卡提到最前，出循环后因此不必再排序。
            existing.meta = bump(existing.meta.as_str());
            let promoted = existing.clone();
            out.retain(|card| card.album != album);
            out.insert(0, promoted);
            continue;
        }

        if out.len() >= limit {
            break;
        }

        out.push(RecentCard {
            id: track.id,
            album,
            artist: track.artist.clone().unwrap_or_default(),
            meta: "1 首".to_string(),
        });
    }

    out
}

/// 「N 首」→「N+1 首」。只认 `recent_cards` 写出去的格式，认不出的按 1 首重起。
fn bump(meta: &str) -> String {
    let n: usize = meta
        .strip_suffix(" 首")
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(1);
    format!("{} 首", n + 1)
}

fn album_card(group: &AlbumGroup) -> RecentCard {
    RecentCard {
        id: group.representative,
        album: group.title.clone(),
        artist: group.display_artist(),
        meta: format!("{} 首", group.track_count()),
    }
}

/// 「为你推荐」：按固定步长采样，结果可复现。
pub fn recommend_cards(groups: &[AlbumGroup], limit: usize) -> Vec<RecentCard> {
    let groups = recommendable(groups);
    if groups.is_empty() || limit == 0 {
        return Vec::new();
    }
    let stride = (groups.len() / limit).max(1);
    groups
        .into_iter()
        .step_by(stride)
        .take(limit)
        .map(album_card)
        .collect()
}

/// 「热门专辑」：按组内曲目数降序取前 `limit` 张。数量相同保持原有名称序。
/// 只做部分选择：整库几千张专辑时不必为了前 12 张全排序。
pub fn popular_cards(groups: &[AlbumGroup], limit: usize) -> Vec<RecentCard> {
    if limit == 0 {
        return Vec::new();
    }
    let mut ranked: Vec<(usize, &AlbumGroup)> =
        recommendable(groups).into_iter().enumerate().collect();
    if ranked.len() > limit {
        ranked.select_nth_unstable_by(limit - 1, |a, b| {
            b.1.track_count()
                .cmp(&a.1.track_count())
                .then(a.0.cmp(&b.0))
        });
        ranked.truncate(limit);
    }
    ranked.sort_unstable_by(|a, b| {
        b.1.track_count()
            .cmp(&a.1.track_count())
            .then(a.0.cmp(&b.0))
    });
    ranked
        .into_iter()
        .map(|(_, group)| album_card(group))
        .collect()
}

/// 能上首页的专辑分组：排除「未知专辑」这一桶。它是没标签曲目的归处，又因为收得多而常排最前。
/// 只在这一层排除，侧栏与「浏览」里它照样在。
fn recommendable(groups: &[AlbumGroup]) -> Vec<&AlbumGroup> {
    groups
        .iter()
        .filter(|group| group.title.as_str() != lyre_core::UNKNOWN_ALBUM)
        .collect()
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`crate::platform::*` 带进来的 gpui `test` 宏会遮蔽内置 `#[test]`。
    use super::{RecentCard, popular_cards, recent_cards, recommend_cards};
    use lyre_core::{Track, TrackId};

    fn track(id: u64, title: &str, album: Option<&str>, artist: Option<&str>) -> Track {
        Track {
            id: TrackId(id),
            path: std::path::PathBuf::from(format!("/music/{id}.mp3")),
            title: title.to_string(),
            artist: artist.map(ToOwned::to_owned),
            album: album.map(ToOwned::to_owned),
            duration: None,
            has_cover: false,
            audio: lyre_core::AudioInfo::default(),
        }
    }

    #[test]
    fn each_album_appears_once() {
        let tracks = vec![
            track(1, "a", Some("城市漫游"), Some("林一")),
            track(2, "b", Some("城市漫游"), Some("林一")),
            track(3, "c", Some("城市漫游"), Some("林一")),
        ];
        let cards = recent_cards(&tracks, 6);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].album, "城市漫游");
        assert_eq!(cards[0].meta, "3 首");
    }

    #[test]
    fn most_recent_album_comes_first() {
        let tracks = vec![
            track(1, "a", Some("最近的"), None),
            track(2, "b", Some("较远的"), None),
        ];
        let cards = recent_cards(&tracks, 6);
        assert_eq!(cards[0].album, "最近的");
        assert_eq!(cards[1].album, "较远的");
    }

    #[test]
    fn repeats_do_not_scramble_the_order() {
        let tracks = vec![
            track(1, "a", Some("A"), None),
            track(2, "b", Some("B"), None),
            track(3, "c", Some("A"), None),
        ];
        let cards = recent_cards(&tracks, 6);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].album, "A");
        assert_eq!(cards[1].album, "B");
        assert_eq!(cards[0].meta, "2 首");
    }

    #[test]
    fn limit_is_respected() {
        let tracks = vec![
            track(1, "a", Some("A"), None),
            track(2, "b", Some("B"), None),
            track(3, "c", Some("C"), None),
        ];
        assert_eq!(recent_cards(&tracks, 2).len(), 2);
    }

    #[test]
    fn missing_album_falls_back_to_unknown_not_empty() {
        let tracks = vec![track(1, "a", None, None)];
        let cards = recent_cards(&tracks, 6);
        assert_eq!(cards[0].album, lyre_core::UNKNOWN_ALBUM);
        assert!(cards[0].artist.is_empty());
    }

    #[test]
    fn blank_album_name_counts_as_unknown() {
        let tracks = vec![track(1, "a", Some("   "), None)];
        let cards = recent_cards(&tracks, 6);
        assert_eq!(cards[0].album, lyre_core::UNKNOWN_ALBUM);
    }

    #[test]
    fn empty_input_yields_no_cards() {
        let cards: Vec<RecentCard> = recent_cards(&[], 6);
        assert!(cards.is_empty());
    }

    #[test]
    fn popular_orders_by_track_count_desc() {
        let tracks = vec![
            track(0, "a", Some("少"), None),
            track(1, "b", Some("多"), None),
            track(2, "c", Some("多"), None),
            track(3, "d", Some("多"), None),
        ];
        let groups = lyre_core::group_by_album(&tracks);
        let cards = popular_cards(&groups, 12);
        assert_eq!(cards[0].album, "多");
        assert_eq!(cards[0].meta, "3 首");
        assert_eq!(cards[1].album, "少");
    }

    #[test]
    fn popular_ties_keep_the_original_album_order_after_partial_selection() {
        let tracks = vec![
            track(0, "a", Some("乙"), None),
            track(1, "b", Some("甲"), None),
            track(2, "c", Some("丙"), None),
            track(3, "d", Some("甲"), None),
            track(4, "e", Some("乙"), None),
        ];
        let groups = lyre_core::group_by_album(&tracks);
        let expected: Vec<String> = groups
            .iter()
            .filter(|group| {
                group.title.as_str() != lyre_core::UNKNOWN_ALBUM && group.track_count() == 2
            })
            .take(2)
            .map(|group| group.title.clone())
            .collect();
        let cards = popular_cards(&groups, 2);

        assert_eq!(cards.len(), 2);
        assert_eq!(
            cards
                .iter()
                .map(|card| card.album.as_str())
                .collect::<Vec<_>>(),
            expected,
            "并列时保持分组原有顺序"
        );
    }

    #[test]
    fn the_unknown_album_bucket_is_never_recommended() {
        let mut tracks = vec![track(0, "known", Some("一张专辑"), None)];
        tracks.extend((1..6).map(|i| track(i, "loose", None, None)));
        let groups = lyre_core::group_by_album(&tracks);
        assert!(
            groups.iter().any(|g| g.title == lyre_core::UNKNOWN_ALBUM),
            "构造不成立：没有「未知专辑」这一组"
        );

        for (name, cards) in [
            ("热门专辑", popular_cards(&groups, 12)),
            ("为你推荐", recommend_cards(&groups, 12)),
        ] {
            assert!(
                cards
                    .iter()
                    .all(|card| card.album != lyre_core::UNKNOWN_ALBUM),
                "{name} 里出现了「未知专辑」"
            );
        }
        assert_eq!(popular_cards(&groups, 12)[0].album, "一张专辑");
    }

    #[test]
    fn recommend_is_a_bounded_stable_sample() {
        let mut tracks = Vec::new();
        for i in 0..6u64 {
            tracks.push(track(i, "t", Some(format!("专辑{i}").as_str()), None));
        }
        let groups = lyre_core::group_by_album(&tracks);
        let first = recommend_cards(&groups, 3);
        let second = recommend_cards(&groups, 3);
        assert_eq!(first.len(), 3, "limit 生效");
        assert_eq!(first, second, "推荐必须是稳定采样，两次结果一致");
        assert_eq!(recommend_cards(&[], 3).len(), 0);
        assert_eq!(recommend_cards(&groups, 0).len(), 0);
    }
}
