//! 搜索匹配：查询按空白分词、曲目与名称的命中判断、可见行投影。

use lyre_core::Track;

/// 把查询串按空白拆成词元，全部转小写（匹配大小写不敏感）。
pub fn parse_query(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// 一首曲目是否命中全部词元，词元之间是 AND。
pub fn matches(track: &Track, terms: &[String]) -> bool {
    terms.iter().all(|term| matches_term(track, term))
}

/// 一个名称（专辑名 / 艺术家名）是否命中全部词元，规则与 [`fn@matches`] 一致。
pub fn matches_name(name: &str, terms: &[String]) -> bool {
    terms.iter().all(|term| contains_ci(name, term))
}

/// 单个词元是否命中；检索范围是标题 / 艺术家 / 专辑，不含路径。
fn matches_term(track: &Track, term: &str) -> bool {
    contains_ci(&track.title, term)
        || track
            .artist
            .as_deref()
            .is_some_and(|artist| contains_ci(artist, term))
        || track
            .album
            .as_deref()
            .is_some_and(|album| contains_ci(album, term))
}

/// 大小写不敏感的包含判断，全程不分配：每敲一个字符都要过一遍全库三个字段。
fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    // 纯 ASCII 走字节滑窗。
    if haystack.is_ascii() && needle.is_ascii() {
        // `windows` 对「窗口比切片长」是直接 panic，不是返回空迭代器
        return needle.len() <= haystack.len()
            && haystack
                .as_bytes()
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()));
    }
    // 含非 ASCII 时按字符边界滑：按字节推进会切进一个中日韩字符中间。
    haystack
        .char_indices()
        .any(|(offset, _)| starts_with_ci(&haystack[offset..], needle))
}

/// `haystack` 的开头是否大小写不敏感地就是 `needle`（`needle` 非空）。
/// 两侧都即时小写化；`flat_map(char::to_lowercase)` 不分配，且能处理一字多字符的小写化（`İ`）。
fn starts_with_ci(haystack: &str, needle: &str) -> bool {
    let mut left = haystack.chars().flat_map(char::to_lowercase);
    let mut right = needle.chars().flat_map(char::to_lowercase);
    loop {
        match (left.next(), right.next()) {
            // 右端（needle）先走完就是命中；左端先走完与字符不等都落到最后一个分支
            (_, None) => return true,
            (Some(a), Some(b)) if a == b => {}
            _ => return false,
        }
    }
}

/// 计算可见投影。`None` 表示全部可见（调用方据此免分配）；`Some(rows)` 是命中曲目在 `tracks` 里的下标，保持原顺序。
pub fn project(tracks: &[Track], terms: &[String]) -> Option<Vec<usize>> {
    project_in(tracks, terms, None)
}

/// 在作用域 `base`（如「正在看哪张专辑」）上叠加词元过滤：两者取交集，不是互相覆盖。
pub fn project_in(
    tracks: &[Track],
    terms: &[String],
    base: Option<&[usize]>,
) -> Option<Vec<usize>> {
    let iter: Box<dyn Iterator<Item = usize> + '_> = match base {
        Some(rows) => Box::new(rows.iter().copied()),
        None => Box::new(0..tracks.len()),
    };

    if terms.is_empty() {
        // 无词元时照搬作用域；无作用域才是「全部可见」
        return base.map(<[usize]>::to_vec);
    }

    Some(
        iter.filter(|index| {
            tracks
                .get(*index)
                .is_some_and(|track| matches(track, terms))
        })
        .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::{contains_ci, matches, parse_query, project, project_in};
    use lyre_core::Track;
    use std::path::PathBuf;

    fn track(title: &str, artist: Option<&str>, album: Option<&str>) -> Track {
        let path = PathBuf::from(format!("/music/{title}.flac"));
        let mut t = Track::fallback(&path);
        t.title = title.to_owned();
        t.artist = artist.map(ToOwned::to_owned);
        t.album = album.map(ToOwned::to_owned);
        t
    }

    fn library() -> Vec<Track> {
        vec![
            track("夜航西飞", Some("陈屿"), Some("城市漫游")),
            track("纸飞机与旧磁带", Some("陈屿"), Some("旧磁带")),
            track("潮汐", None, None),
            track("Midnight Drive", Some("Kaito"), Some("Neon")),
        ]
    }

    #[test]
    fn query_splits_on_whitespace_and_lowercases() {
        assert_eq!(parse_query("  陈屿   城市 "), vec!["陈屿", "城市"]);
        assert_eq!(parse_query("Neon DRIVE"), vec!["neon", "drive"]);
        assert!(parse_query("   ").is_empty());
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(contains_ci("Midnight Drive", "drive"));
        assert!(contains_ci("Midnight Drive", "MIDNIGHT"));
        assert!(contains_ci("Neon", "neon"));
        assert!(!contains_ci("Neon", "night"));
    }

    #[test]
    fn matching_handles_multibyte_text_safely() {
        assert!(contains_ci("夜航西飞", "西飞"));
        assert!(contains_ci("夜航西飞", "夜"));
        assert!(!contains_ci("夜航西飞", "东"));
        assert!(contains_ci("Naïve", "naïve"));
        assert!(contains_ci("a🎵b", "🎵b"));
    }

    /// 词元比字段长必然不匹配：ASCII 分支的字节滑窗在这里会 panic。
    #[test]
    fn a_needle_longer_than_the_haystack_never_matches() {
        assert!(!contains_ci("Neon", "neon drive"));
        assert!(!contains_ci("夜航", "夜航西飞"));
        assert!(contains_ci("Neon", ""));
    }

    #[test]
    fn ascii_term_hits_a_multibyte_field() {
        assert!(contains_ci("夜航 Neon 西飞", "neon"));
        assert!(contains_ci("夜航 Neon 西飞", "NEON"));
        assert!(!contains_ci("夜航 Neon 西飞", "expo"));
    }

    #[test]
    fn terms_are_combined_with_and_not_or() {
        let t = track("夜航西飞", Some("陈屿"), Some("城市漫游"));
        assert!(matches(&t, &parse_query("陈屿 城市")));
        assert!(matches(&t, &parse_query("夜航 漫游")));
        assert!(!matches(&t, &parse_query("陈屿 不存在")));
    }

    #[test]
    fn title_artist_and_album_are_all_searchable() {
        let t = track("潮汐", Some("海边"), Some("蓝色专辑"));
        assert!(matches(&t, &parse_query("潮汐")));
        assert!(matches(&t, &parse_query("海边")));
        assert!(matches(&t, &parse_query("蓝色")));
    }

    #[test]
    fn name_matching_uses_the_same_and_case_insensitive_rules() {
        assert!(super::matches_name("城市漫游", &parse_query("城市")));
        assert!(super::matches_name("Neon", &parse_query("neon")));
        assert!(!super::matches_name("Neon", &parse_query("neon drive")));
        assert!(super::matches_name(
            "Neon Drive",
            &parse_query("neon drive")
        ));
    }

    #[test]
    fn missing_fields_do_not_panic() {
        let t = track("潮汐", None, None);
        assert!(!matches(&t, &parse_query("陈屿")));
        assert!(matches(&t, &parse_query("潮汐")));
    }

    #[test]
    fn empty_query_means_no_projection_at_all() {
        assert!(project(&library(), &parse_query("")).is_none());
    }

    #[test]
    fn projection_keeps_library_order() {
        let tracks = library();
        let rows = project(&tracks, &parse_query("陈屿")).unwrap();
        assert_eq!(rows, vec![0, 1], "命中项的次序必须与曲库一致");

        let rows = project(&tracks, &parse_query("neon")).unwrap();
        assert_eq!(rows, vec![3]);
    }

    #[test]
    fn scope_and_terms_intersect_instead_of_overriding_each_other() {
        let tracks = library();
        let album_rows = [0usize, 1]; // 陈屿的两首
        let base = album_rows.as_slice();

        let scoped = project_in(&tracks, &parse_query(""), Some(base)).unwrap();
        assert_eq!(scoped, vec![0, 1], "无词元时照搬作用域");

        let narrowed = project_in(&tracks, &parse_query("夜航"), Some(base)).unwrap();
        assert_eq!(narrowed, vec![0], "搜索应当在作用域内收窄");

        let outside = project_in(&tracks, &parse_query("Neon"), Some(base)).unwrap();
        assert!(
            outside.is_empty(),
            "作用域外的曲目即使命中词元也不能出现在结果里"
        );
    }

    #[test]
    fn no_scope_and_no_terms_still_means_none() {
        let rows = project_in(&library(), &parse_query(""), None);
        assert!(rows.is_none(), "两个条件都没有时不该分配");
    }

    #[test]
    fn projection_can_be_empty() {
        let rows = project(&library(), &parse_query("不存在的东西")).unwrap();
        assert!(rows.is_empty(), "全部不命中时应当是空投影而不是 None");
    }
}
