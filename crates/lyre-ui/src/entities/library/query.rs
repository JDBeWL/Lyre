//! 搜索过滤与可见行投影：`visible` 存可见曲目在 `playlist.tracks()` 里的下标，无过滤时为 `None`。
//! 改变曲目集合或作用域的地方都要调 [`LibraryEntity::recompute_visible`]。

use lyre_core::{Track, TrackId, album_key, artist_keys};

use crate::entities::filter;
use crate::platform::*;

use super::LibraryEntity;
use super::scope::Scope;

/// 把请求区间夹进可见行数以内：投影可能在列表取数据之前刚收窄，两端都要夹。
fn clamp_rows(range: std::ops::Range<usize>, len: usize) -> std::ops::Range<usize> {
    let end = range.end.min(len);
    range.start.min(end)..end
}

/// 这个查询串算不算「用户在搜索」。纯空白不算：`parse_query` 拆出空词表、等价于没搜，两边口径要一致。
pub fn searching(query: &str) -> bool {
    !query.trim().is_empty()
}

impl LibraryEntity {
    pub fn query(&self) -> &str {
        &self.query
    }

    /// 投影是否生效（作用域收窄或有搜索词）。名字易误读成「用户在搜索吗」，那个语义用 [`Self::has_query`]。
    pub fn is_filtering(&self) -> bool {
        self.visible.is_some()
    }

    /// 用户是否输入了搜索词：判据是 `query` 而非 `visible`，只进了一张专辑时为 `false`。
    pub fn has_query(&self) -> bool {
        searching(&self.query)
    }

    /// 可见行数；无投影时等于曲库长度。列表用它作 `uniform_list` 的 `item_count`。
    pub fn visible_len(&self) -> usize {
        match &self.visible {
            Some(rows) => rows.len(),
            None => self.playlist.len(),
        }
    }

    /// 取可见区间的曲目，只克隆这一段。
    pub fn visible_tracks(&self, range: std::ops::Range<usize>) -> Vec<Track> {
        let std::ops::Range { start, end } = clamp_rows(range, self.visible_len());
        match &self.visible {
            Some(rows) => {
                let tracks = self.playlist.tracks();
                rows[start..end]
                    .iter()
                    .filter_map(|index| tracks.get(*index).cloned())
                    .collect()
            }
            None => self.playlist.tracks()[start..end].to_vec(),
        }
    }

    /// 取可见区间的曲目 id（不 clone `Track`）。
    pub fn visible_ids(&self, range: std::ops::Range<usize>) -> Vec<TrackId> {
        let std::ops::Range { start, end } = clamp_rows(range, self.visible_len());
        match &self.visible {
            Some(rows) => {
                let tracks = self.playlist.tracks();
                rows[start..end]
                    .iter()
                    .filter_map(|index| tracks.get(*index))
                    .map(|track| track.id)
                    .collect()
            }
            None => self.playlist.tracks()[start..end]
                .iter()
                .map(|track| track.id)
                .collect(),
        }
    }

    /// 光标所在的可见行号。
    pub fn focus_row(&self) -> Option<usize> {
        self.focus.and_then(|id| self.row_of(id))
    }

    /// 某首曲目落在第几行（可见投影内）。无投影时就是曲库下标。
    pub fn row_of(&self, id: TrackId) -> Option<usize> {
        match &self.visible {
            Some(rows) => {
                let library_index = self.playlist.index_of(id)?;
                rows.iter().position(|i| *i == library_index)
            }
            None => self.playlist.index_of(id),
        }
    }

    /// 设置搜索词并重算投影。搜索只改变哪些行可见，不改播放队列。
    pub fn set_query(&mut self, query: impl Into<String>, cx: &mut Context<Self>) {
        let query = query.into();
        if self.query == query {
            return;
        }
        self.query = query;
        self.recompute_visible(cx);
        cx.notify();
    }

    /// 重算投影（作用域 ∩ 搜索词），并把光标修正到仍然可见的位置。
    pub(super) fn recompute_visible(&mut self, cx: &mut Context<Self>) {
        let terms = filter::parse_query(&self.query);
        let scope = self.scope_rows();
        self.visible = filter::project_in(self.playlist.tracks(), &terms, scope.as_deref());

        let still_visible = self.focus.is_some_and(|id| self.row_of(id).is_some());
        if !still_visible {
            let next = match &self.visible {
                Some(rows) => rows
                    .first()
                    .and_then(|index| self.playlist.tracks().get(*index))
                    .map(|t| t.id),
                None => self.playlist.tracks().first().map(|t| t.id),
            };
            self.focus = None; // 先清掉，否则 `set_focus` 会认为「没变化」而跳过
            if let Some(id) = next {
                self.set_focus(id, cx);
            }
            // 系统替用户落的座（换作用域 / 改过滤），不是键盘走出来的。
            self.cursor_visible = false;
        }
    }

    /// 作用域圈定的曲目下标，`All` 返回 `None`。判定必须与分组共用 `lyre_core` 的 `album_key` / `artist_keys`。
    fn scope_rows(&self) -> Option<Vec<usize>> {
        match &self.scope {
            Scope::All => None,
            Scope::Album(title) => Some(
                self.playlist
                    .tracks()
                    .iter()
                    .enumerate()
                    .filter(|(_, track)| album_key(track) == title)
                    .map(|(index, _)| index)
                    .collect(),
            ),
            Scope::Artist(name) => Some(
                self.playlist
                    .tracks()
                    .iter()
                    .enumerate()
                    .filter(|(_, track)| artist_keys(track).iter().any(|key| key == name))
                    .map(|(index, _)| index)
                    .collect(),
            ),
            // 歌单按成员顺序出下标，`project_in` 保持传入顺序，所以拖拽排序的结果就是这里看到的顺序；解析不出的成员跳过。
            Scope::Collection(id) => Some(
                self.collections
                    .get(*id)?
                    .tracks
                    .iter()
                    .filter_map(|track| self.playlist.index_of(*track))
                    .collect(),
            ),
            // 收藏没有自己的顺序，按曲库当前顺序显示
            Scope::Favorites => Some(
                self.playlist
                    .tracks()
                    .iter()
                    .enumerate()
                    .filter(|(_, track)| self.liked.contains(track.id))
                    .map(|(index, _)| index)
                    .collect(),
            ),
        }
    }

    pub fn clear_query(&mut self, cx: &mut Context<Self>) {
        self.set_query(String::new(), cx);
    }
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`crate::platform::*` 带进来的 gpui `test` 宏会遮蔽内置 `#[test]`
    use super::{clamp_rows, searching};

    /// 两端都要夹：投影收窄后，列表可能还按旧的行数来要数据。
    #[test]
    fn clamp_rows_bounds_both_ends_without_inverting() {
        assert_eq!(clamp_rows(0..5, 10), 0..5, "在范围内时原样");
        assert_eq!(clamp_rows(0..99, 10), 0..10, "右端越界时夹到长度");
        assert_eq!(clamp_rows(8..99, 10), 8..10, "左端该保留");
        assert_eq!(
            clamp_rows(50..99, 10),
            10..10,
            "左端也越界时收成空区间，不能反成 10..50"
        );

        let inverted = std::ops::Range { start: 9, end: 3 };
        assert_eq!(clamp_rows(inverted, 10), 3..3, "start > end 时不能 panic");
    }

    #[test]
    fn a_real_query_counts_as_searching() {
        assert!(searching("夜航"));
        assert!(searching("Neon"));
        assert!(searching("陈屿 城市"));
    }

    #[test]
    fn pure_whitespace_is_not_searching() {
        assert!(!searching(""));
        assert!(!searching(" "));
        assert!(!searching("   \t\n "));
    }

    #[test]
    fn whitespace_around_a_real_query_still_counts() {
        assert!(searching(" 夜航 "));
        assert!(searching("\tNeon\n"));
    }
}
