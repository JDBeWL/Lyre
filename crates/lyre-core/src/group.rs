//! 按专辑 / 艺术家聚合成分组视图，纯函数 `&[Track] -> Vec<AlbumGroup>`。
//! 专辑只按专辑名分组，不按 `(专辑名, 艺术家)`：真实曲库里 feat. 会把一张专辑拆成十几张。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::track::{Track, TrackId};

pub const UNKNOWN_ALBUM: &str = "未知专辑";
pub const UNKNOWN_ARTIST: &str = "未知艺术家";
pub const VARIOUS_ARTISTS: &str = "群星";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlbumGroup {
    /// 专辑名，永远非空 —— 无标签曲目归入 [`UNKNOWN_ALBUM`]。
    pub title: String,
    /// 组内出现过的艺术家，去重后按首次出现顺序排列。
    pub artists: Vec<String>,
    /// 组内曲目 id，顺序与输入一致（`Track` 没有唱片序号字段）。
    pub tracks: Vec<TrackId>,
    /// 用来取封面的那首曲目，曲目不为空时必有值。
    pub representative: TrackId,
    /// 组内曲目是否全部都有时长，见 [`AlbumGroup::total_duration`]。
    all_durations_known: bool,
    total: Duration,
    /// `representative` 是否带封面，决定后续曲目能否顶替它。
    rep_has_cover: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArtistGroup {
    /// 艺术家名，永远非空，且永远是拆分后的单个人（见 [`artist_keys`]）。
    pub name: String,
    /// 名下专辑名，去重后按首次出现顺序排列。
    pub albums: Vec<String>,
    pub tracks: Vec<TrackId>,
    pub representative: TrackId,
    all_durations_known: bool,
    total: Duration,
    rep_has_cover: bool,
}

impl AlbumGroup {
    /// 艺术家显示口径：0 位 → 空串，1 位 → 原名，多位 → [`VARIOUS_ARTISTS`]。
    pub fn display_artist(&self) -> String {
        match self.artists.as_slice() {
            [] => String::new(),
            [only] => only.clone(),
            _ => VARIOUS_ARTISTS.to_string(),
        }
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// 组内总时长：只有每首时长都已知才返回 `Some`，偏小的和会被 UI 当成精确值显示。
    pub fn total_duration(&self) -> Option<Duration> {
        if self.all_durations_known {
            Some(self.total)
        } else {
            None
        }
    }
}

impl ArtistGroup {
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    pub fn album_count(&self) -> usize {
        self.albums.len()
    }

    pub fn total_duration(&self) -> Option<Duration> {
        if self.all_durations_known {
            Some(self.total)
        } else {
            None
        }
    }
}

/// 每收到一首曲目要做的三件事（两个分组结构体同构）：累计时长、选出封面代表、收集曲目 id。
struct Accumulator<'a> {
    tracks: &'a mut Vec<TrackId>,
    representative: &'a mut TrackId,
    rep_has_cover: &'a mut bool,
    total: &'a mut Duration,
    all_durations_known: &'a mut bool,
}

trait Accumulate {
    /// 借出那五个字段。两个结构体的字段同名同型，各自的实现只需一一对应地借出来。
    fn accumulator(&mut self) -> Accumulator<'_>;

    /// 收下一首曲目。不会重复收到同一首 —— 调用方是按分组键循环的。
    fn absorb(&mut self, track: &Track) {
        let Accumulator {
            tracks,
            representative,
            rep_has_cover,
            total,
            all_durations_known,
        } = self.accumulator();

        match track.duration {
            Some(duration) => *total += duration,
            None => *all_durations_known = false,
        }

        // 组内第一首带封面的曲目胜过不带封面的；一首都不带时退回第一首
        if tracks.is_empty() || (track.has_cover && !*rep_has_cover) {
            *representative = track.id;
            *rep_has_cover = track.has_cover;
        }

        tracks.push(track.id);
    }
}

impl Accumulate for AlbumGroup {
    fn accumulator(&mut self) -> Accumulator<'_> {
        Accumulator {
            tracks: &mut self.tracks,
            representative: &mut self.representative,
            rep_has_cover: &mut self.rep_has_cover,
            total: &mut self.total,
            all_durations_known: &mut self.all_durations_known,
        }
    }
}

impl Accumulate for ArtistGroup {
    fn accumulator(&mut self) -> Accumulator<'_> {
        Accumulator {
            tracks: &mut self.tracks,
            representative: &mut self.representative,
            rep_has_cover: &mut self.rep_has_cover,
            total: &mut self.total,
            all_durations_known: &mut self.all_durations_known,
        }
    }
}

/// 按专辑名聚合。结果按专辑名升序，[`UNKNOWN_ALBUM`] 永远在最后。
pub fn group_by_album(tracks: &[Track]) -> Vec<AlbumGroup> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, AlbumGroup> =
        std::collections::HashMap::new();

    for track in tracks {
        let title = album_key(track);
        let group = groups.entry(title.to_string()).or_insert_with(|| {
            order.push(title.to_string());
            AlbumGroup {
                title: title.to_string(),
                artists: Vec::new(),
                tracks: Vec::new(),
                representative: track.id,
                all_durations_known: true,
                total: Duration::ZERO,
                rep_has_cover: false,
            }
        });

        // 专辑组多记一样东西：组内出现过的艺术家（去重，按首次出现顺序）
        if let Some(artist) = non_empty(track.artist.as_deref())
            && !group.artists.iter().any(|a| a == artist)
        {
            group.artists.push(artist.to_string());
        }

        group.absorb(track);
    }

    // `order` 就是按这个 map 的键攒出来的，两边一一对应；`remove` 顺手把 map 抽干，
    // 所以不需要「取不到就 panic」的兜底。
    let mut result: Vec<AlbumGroup> = order
        .into_iter()
        .filter_map(|title| groups.remove(&title))
        .collect();
    sort_unknown_last(&mut result, |g| g.title.as_str(), UNKNOWN_ALBUM);
    result
}

/// 按艺术家聚合，结果升序，[`UNKNOWN_ARTIST`] 永远在最后。
/// 一首合作曲进每一位名家的组（见 [`artist_keys`]），所以各组曲目数之和大于曲库总数。
pub fn group_by_artist(tracks: &[Track]) -> Vec<ArtistGroup> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, ArtistGroup> =
        std::collections::HashMap::new();

    for track in tracks {
        for name in artist_keys(track) {
            let group = groups.entry(name.to_string()).or_insert_with(|| {
                order.push(name.to_string());
                ArtistGroup {
                    name: name.to_string(),
                    albums: Vec::new(),
                    tracks: Vec::new(),
                    representative: track.id,
                    all_durations_known: true,
                    total: Duration::ZERO,
                    rep_has_cover: false,
                }
            });

            // 艺术家组多记一样东西：名下出现过的专辑（去重，按首次出现顺序）
            if let Some(album) = non_empty(track.album.as_deref())
                && !group.albums.iter().any(|a| a == album)
            {
                group.albums.push(album.to_string());
            }

            group.absorb(track);
        }
    }

    let mut result: Vec<ArtistGroup> = order
        .into_iter()
        .filter_map(|name| groups.remove(&name))
        .collect();
    sort_unknown_last(&mut result, |g| g.name.as_str(), UNKNOWN_ARTIST);
    result
}

/// `Some(s)` 当且仅当 `s` trim 后非空 —— 标签里有字段但全是空格是常见脏数据。
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

/// 这首曲目属于哪张专辑，永远非空。UI 筛组内曲目必须与 [`group_by_album`] 用同一套判定。
pub fn album_key(track: &Track) -> &str {
    album_key_str(track.album.as_deref())
}

/// 单个专辑名字段的分组键，判定与 [`album_key`] 一致（UI 手上只有字段而非整首曲目时用）。
pub fn album_key_str(album: Option<&str>) -> &str {
    non_empty(album).unwrap_or(UNKNOWN_ALBUM)
}

/// 一个艺术家字段里的多人分隔符。不含顿号与全角逗号：`25時、ナイトコードで。` 是一个组合的正式名字。
pub const ARTIST_SEPARATORS: [char; 2] = ['/', ','];

/// 这首曲目名下的每一位艺术家，至少一位（全空退成 [`UNKNOWN_ARTIST`]）。
/// 分组与「点进某位艺术家」的筛选都必须走这里。
pub fn artist_keys(track: &Track) -> Vec<&str> {
    artist_keys_str(track.artist.as_deref())
}

/// 见 [`artist_keys`]。字段值是 `Option<String>` 的场合（列表行、菜单）直接用它。
pub fn artist_keys_str(artist: Option<&str>) -> Vec<&str> {
    let mut keys: Vec<&str> = non_empty(artist)
        .unwrap_or("")
        .split(ARTIST_SEPARATORS)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if keys.is_empty() {
        keys.push(UNKNOWN_ARTIST);
    }
    keys
}

/// 见 [`album_key_str`]。返回的是第一位艺术家，只用来回答「这个字段里到底有没有名字」
/// 和取一个默认落点；分组与筛选要用 [`artist_keys_str`]，否则多人曲目会漏进后面的名家。
pub fn artist_key_str(artist: Option<&str>) -> &str {
    non_empty(artist)
        .unwrap_or("")
        .split(ARTIST_SEPARATORS)
        .map(str::trim)
        .find(|part| !part.is_empty())
        .unwrap_or(UNKNOWN_ARTIST)
}

/// 排序：`unknown` 压到最后，其余按 `key` 升序。
/// 忽略 ASCII 大小写，与曲目列表同口径；非 ASCII 仍按码位序。
fn sort_unknown_last<T, F>(groups: &mut [T], key: F, unknown: &str)
where
    F: Fn(&T) -> &str,
{
    groups.sort_by(|a, b| {
        let (a_unknown, b_unknown) = (key(a) == unknown, key(b) == unknown);
        match (a_unknown, b_unknown) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => {
                let (a_key, b_key) = (key(a), key(b));
                let folded = a_key
                    .chars()
                    .flat_map(char::to_lowercase)
                    .cmp(b_key.chars().flat_map(char::to_lowercase));
                // 只有大小写不同的两个名字（「Aika」与「AIKA」）折叠后相等，
                // 再按原样比一次定序，否则每帧重排都会互换位置。
                folded.then_with(|| a_key.cmp(b_key))
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn track(name: &str) -> Track {
        let mut t = Track::fallback(&PathBuf::from(format!("/music/{name}.mp3")));
        t.title = name.to_string();
        t
    }

    fn with(path: &str, artist: Option<&str>, album: Option<&str>) -> Track {
        let mut t = track(path);
        t.artist = artist.map(ToString::to_string);
        t.album = album.map(ToString::to_string);
        t
    }

    #[test]
    fn same_album_title_merges_into_one_group() {
        let tracks = vec![
            with("a1", Some("陈屿"), Some("城市漫游")),
            with("a2", Some("陈屿"), Some("城市漫游")),
            with("b1", Some("低苦艾"), Some("兰州兰州")),
        ];

        let groups = group_by_album(&tracks);
        assert_eq!(groups.len(), 2, "两张不同的专辑");

        let roaming = groups
            .iter()
            .find(|g| g.title == "城市漫游")
            .expect("应当存在");
        assert_eq!(roaming.track_count(), 2, "同名专辑的两首应当在同一组");
        assert!(groups.iter().any(|g| g.title == "兰州兰州"));
    }

    #[test]
    fn groups_sort_by_unicode_codepoint_not_pinyin() {
        // 已知限制：中文按 Unicode 码点排序，不是拼音序；拼音序要付 ICU 级排序表的成本
        let tracks = vec![
            with("a", Some("陈屿"), Some("城市漫游")),
            with("b", Some("低苦艾"), Some("兰州兰州")),
            with("c", Some("张震岳"), Some("阿波罗")),
        ];

        let titles: Vec<String> = group_by_album(&tracks)
            .into_iter()
            .map(|g| g.title)
            .collect();
        assert_eq!(titles, ["兰州兰州", "城市漫游", "阿波罗"]);
    }

    /// 分组列表与曲目列表必须用同一种序，否则「BUZZ CUTZ 排在 Blue Archive 前面」
    /// 这种一眼看上去就是排序坏了的顺序会留在侧栏里。
    #[test]
    fn group_order_ignores_ascii_case() {
        let tracks = vec![
            with("a", Some("x"), Some("BUZZ CUTZ")),
            with("b", Some("x"), Some("Blue Archive")),
            with("c", Some("x"), Some("apple")),
        ];
        let titles: Vec<String> = group_by_album(&tracks)
            .into_iter()
            .map(|g| g.title)
            .collect();
        assert_eq!(titles, ["apple", "Blue Archive", "BUZZ CUTZ"]);
    }

    /// 只有大小写不同的两个组名折叠后相等，必须再按原样定一次序，
    /// 否则它们在每次重排时互换位置。
    #[test]
    fn names_differing_only_in_case_have_a_stable_order() {
        let tracks = vec![
            with("a", Some("x"), Some("Aika")),
            with("b", Some("x"), Some("AIKA")),
        ];
        let forward: Vec<String> = group_by_album(&tracks)
            .into_iter()
            .map(|g| g.title)
            .collect();
        let backward: Vec<Track> = tracks.into_iter().rev().collect();
        let reversed: Vec<String> = group_by_album(&backward)
            .into_iter()
            .map(|g| g.title)
            .collect();
        assert_eq!(forward, reversed);
        assert_eq!(forward, ["AIKA", "Aika"]);
    }

    #[test]
    fn group_order_follows_input_order() {
        let tracks = vec![
            with("03 第三首", Some("陈屿"), Some("城市漫游")),
            with("01 第一首", Some("陈屿"), Some("城市漫游")),
            with("02 第二首", Some("陈屿"), Some("城市漫游")),
        ];

        let groups = group_by_album(&tracks);
        assert_eq!(
            groups[0].tracks,
            [tracks[0].id, tracks[1].id, tracks[2].id],
            "顺序应当与输入一致，而不是被重排"
        );
    }

    #[test]
    fn same_album_title_with_different_artists_stays_one_group() {
        // 专辑名相同、艺术家不同必须合并，否则 feat. 会把一张专辑拆成十几张
        let tracks = vec![
            with("a", Some("陈屿"), Some("群星合辑")),
            with("b", Some("低苦艾"), Some("群星合辑")),
            with("c", Some("陈屿"), Some("群星合辑")),
        ];

        let groups = group_by_album(&tracks);
        assert_eq!(groups.len(), 1, "同名专辑必须合并成一组");
        assert_eq!(groups[0].artists, ["陈屿", "低苦艾"], "艺术家去重且保序");
        assert_eq!(groups[0].display_artist(), "群星");
    }

    #[test]
    fn single_artist_shows_the_name_not_various() {
        let tracks = vec![with("a", Some("陈屿"), Some("城市漫游"))];
        let groups = group_by_album(&tracks);
        assert_eq!(groups[0].display_artist(), "陈屿");
    }

    #[test]
    fn no_artist_at_all_yields_an_empty_label() {
        let tracks = vec![with("a", None, Some("城市漫游"))];
        let groups = group_by_album(&tracks);
        assert_eq!(groups[0].display_artist(), "");
    }

    #[test]
    fn untagged_tracks_land_in_unknown_album_at_the_end() {
        // 无标签的排在最后，否则淹没正常专辑
        let tracks = vec![
            with("z", None, None),
            with("a", Some("陈屿"), Some("阿波罗")),
            with("y", Some("低苦艾"), Some("兰州兰州")),
        ];

        let titles: Vec<String> = group_by_album(&tracks)
            .into_iter()
            .map(|g| g.title)
            .collect();
        assert_eq!(titles, ["兰州兰州", "阿波罗", UNKNOWN_ALBUM]);
        assert_eq!(titles.last().map(String::as_str), Some(UNKNOWN_ALBUM));
    }

    #[test]
    fn whitespace_only_tags_count_as_untagged() {
        let tracks = vec![with("a", Some("   "), Some("  "))];
        let groups = group_by_album(&tracks);
        assert_eq!(groups[0].title, UNKNOWN_ALBUM);
        assert_eq!(groups[0].display_artist(), "");
    }

    #[test]
    fn representative_prefers_the_first_track_with_a_cover() {
        let mut tracks = vec![
            with("a", Some("陈屿"), Some("城市漫游")),
            with("b", Some("陈屿"), Some("城市漫游")),
            with("c", Some("陈屿"), Some("城市漫游")),
        ];
        tracks[2].has_cover = true;

        let groups = group_by_album(&tracks);
        assert_eq!(
            groups[0].representative, tracks[2].id,
            "应当取组内第一首带封面的曲目"
        );
    }

    #[test]
    fn representative_falls_back_to_the_first_track_when_nothing_has_a_cover() {
        let tracks = vec![
            with("a", Some("陈屿"), Some("城市漫游")),
            with("b", Some("陈屿"), Some("城市漫游")),
        ];
        let groups = group_by_album(&tracks);
        assert_eq!(
            groups[0].representative, tracks[0].id,
            "一首都没有封面时退回第一首，UI 走占位图"
        );
    }

    #[test]
    fn total_duration_is_none_unless_every_track_knows_its_length() {
        let all_known = vec![
            with("a", Some("陈屿"), Some("城市漫游")),
            with("b", Some("陈屿"), Some("城市漫游")),
        ];
        let mut tracks = all_known.clone();
        tracks[0].duration = Some(Duration::from_secs(100));
        tracks[1].duration = Some(Duration::from_secs(50));
        assert_eq!(
            group_by_album(&tracks)[0].total_duration(),
            Some(Duration::from_secs(150))
        );

        let mut partial = tracks.clone();
        partial[1].duration = None;
        assert_eq!(
            group_by_album(&partial)[0].total_duration(),
            None,
            "缺一首时长就不给出总数"
        );
    }

    #[test]
    fn artist_groups_count_albums_and_tracks() {
        let tracks = vec![
            with("a", Some("陈屿"), Some("城市漫游")),
            with("b", Some("陈屿"), Some("城市漫游")),
            with("c", Some("陈屿"), Some("夜航西飞")),
            with("d", Some("低苦艾"), Some("兰州兰州")),
        ];

        let groups = group_by_artist(&tracks);
        assert_eq!(groups.len(), 2);

        let chen = groups.iter().find(|g| g.name == "陈屿").expect("应当存在");
        assert_eq!(chen.track_count(), 3);
        assert_eq!(chen.album_count(), 2, "名下有两张不同专辑");
        assert_eq!(chen.albums, ["城市漫游", "夜航西飞"], "专辑名去重且保序");
        assert!(groups.iter().any(|g| g.name == "低苦艾"));
    }

    #[test]
    fn unknown_artist_sorts_last_too() {
        let tracks = vec![with("z", None, Some("x")), with("a", Some("陈屿"), None)];
        let groups = group_by_artist(&tracks);
        assert_eq!(groups[0].name, "陈屿");
        assert_eq!(groups[1].name, UNKNOWN_ARTIST);
    }

    #[test]
    fn grouping_keys_agree_with_the_group_titles() {
        // UI 用 album_key 筛选组内曲目，两侧一旦漂移就表现为「少了几首」而不是报错
        let mut tracks = vec![
            with("a", Some("陈屿"), Some("城市漫游")),
            with("b", Some("陈屿"), Some("  城市漫游  ")), // 标签脏：前后空格
            with("c", Some("陈屿"), Some("  ")),           // 标签脏：纯空格
            with("d", None, None),
        ];
        tracks[1].artist = Some("陈屿".into());

        let groups = group_by_album(&tracks);
        for group in &groups {
            let members: Vec<TrackId> = tracks
                .iter()
                .filter(|t| album_key(t) == group.title)
                .map(|t| t.id)
                .collect();
            assert_eq!(
                members, group.tracks,
                "「{}」用 album_key 筛出来的集合必须与该分组完全一致",
                group.title
            );
        }

        let artists = group_by_artist(&tracks);
        for group in &artists {
            let members: Vec<TrackId> = tracks
                .iter()
                .filter(|t| artist_keys(t).contains(&group.name.as_str()))
                .map(|t| t.id)
                .collect();
            assert_eq!(
                members, group.tracks,
                "「{}」的 artist_keys 筛选不一致",
                group.name
            );
        }
    }

    #[test]
    fn a_collaboration_counts_towards_every_artist() {
        let tracks = vec![
            with("a", Some("陈屿/低苦艾"), Some("城市漫游")),
            with("b", Some("陈屿"), Some("夜航西飞")),
        ];

        let groups = group_by_artist(&tracks);
        let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            ["低苦艾", "陈屿"],
            "斜杠里的两个人各自成组，而不是「陈屿/低苦艾」这一整串"
        );

        let chen = groups.iter().find(|g| g.name == "陈屿").expect("应当存在");
        assert_eq!(chen.track_count(), 2, "两首都有陈屿");
        let dikuye = groups
            .iter()
            .find(|g| g.name == "低苦艾")
            .expect("应当存在");
        assert_eq!(dikuye.track_count(), 1);
        assert_eq!(dikuye.albums, ["城市漫游"]);
    }

    #[test]
    fn ascii_comma_splits_too_but_the_ideographic_comma_does_not() {
        // `25時、ナイトコードで。` 是一个组合的正式名字，按顿号切会劈出「25時」这种假艺术家
        let tracks = vec![
            with("a", Some("初星学園, SHiNTA"), Some("x")),
            with(
                "b",
                Some("カンザキイオリ/25時、ナイトコードで。"),
                Some("y"),
            ),
        ];

        let names: Vec<String> = group_by_artist(&tracks)
            .into_iter()
            .map(|g| g.name)
            .collect();
        assert_eq!(
            names,
            [
                "25時、ナイトコードで。".to_owned(),
                "SHiNTA".to_owned(),
                "カンザキイオリ".to_owned(),
                "初星学園".to_owned(),
            ]
        );
    }

    #[test]
    fn splitting_trims_and_drops_empty_pieces() {
        assert_eq!(artist_keys_str(Some(" 甲 / 乙 ,, 丙 ")), ["甲", "乙", "丙"]);
        assert_eq!(artist_keys_str(Some("///")), [UNKNOWN_ARTIST]);
        assert_eq!(artist_keys_str(None), [UNKNOWN_ARTIST]);
        // 「有没有艺术家」看的是拆分后的第一位，而不是整串
        assert_eq!(artist_key_str(Some("甲/乙")), "甲");
        assert_eq!(artist_key_str(Some("  / ")), UNKNOWN_ARTIST);
    }

    #[test]
    fn empty_library_yields_no_groups() {
        assert!(group_by_album(&[]).is_empty());
        assert!(group_by_artist(&[]).is_empty());
    }

    #[test]
    fn groups_are_deterministic_across_calls() {
        let tracks = vec![
            with("a", Some("陈屿"), Some("城市漫游")),
            with("b", Some("低苦艾"), Some("兰州兰州")),
            with("c", None, None),
        ];
        assert_eq!(group_by_album(&tracks), group_by_album(&tracks));
    }
}
