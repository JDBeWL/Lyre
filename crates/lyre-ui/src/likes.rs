//! 「喜欢」集合，独立于 `Settings` 落盘到 `likes.json`。
//! 读不到退回空集合，写不进只记日志（见 [`crate::persist`]）。

use std::collections::HashSet;

use lyre_core::TrackId;

use crate::persist;

/// 落盘文件名。
const FILE: &str = "likes.json";

/// 用 `HashSet`：「这首喜不喜欢」在每个可见行上每帧都查一次，`Vec` 的线性查找可测量。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Likes {
    ids: HashSet<TrackId>,
}

impl Likes {
    /// 读不到（首次启动、文件损坏、目录不可读）就退回空集合。
    pub fn load() -> Self {
        // `TrackId` 是单字段元组结构体，serde 序列化成内层的 u64，所以文件形如 `[123, 456]`。
        let ids: Vec<TrackId> = persist::load_or_default(FILE);
        Self {
            ids: ids.into_iter().collect(),
        }
    }

    /// 写回磁盘。失败只记日志，不影响调用方。
    pub fn save(&self) {
        // 集合无序，排序后写盘让文件在两次保存之间稳定。
        let mut ids: Vec<TrackId> = self.ids.iter().copied().collect();
        ids.sort_unstable();
        persist::save_lossy(FILE, ids);
    }

    pub fn contains(&self, id: TrackId) -> bool {
        self.ids.contains(&id)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// 取反收藏状态，返回切换后是否已收藏。
    pub fn toggle(&mut self, id: TrackId) -> bool {
        if self.ids.remove(&id) {
            false
        } else {
            self.ids.insert(id);
            true
        }
    }

    /// 批量收藏，返回真正新增的条数 —— 已收藏的不能算进去，否则调用方会以为存盘成功了。
    pub fn insert_all(&mut self, ids: impl IntoIterator<Item = TrackId>) -> usize {
        let before = self.ids.len();
        self.ids.extend(ids);
        self.ids.len() - before
    }
}

#[cfg(test)]
mod tests {
    use super::Likes;
    use lyre_core::TrackId;

    #[test]
    fn toggle_flips_and_reports_the_new_state() {
        let mut likes = Likes::default();
        let id = TrackId(7);

        assert!(!likes.contains(id));
        assert!(likes.toggle(id), "第一次切换应当变成「已收藏」");
        assert!(likes.contains(id));
        assert_eq!(likes.len(), 1);

        assert!(!likes.toggle(id), "第二次切换应当取消收藏");
        assert!(!likes.contains(id));
        assert!(likes.is_empty());
    }

    #[test]
    fn round_trips_through_json() {
        // 落盘形状是 `Vec<TrackId>`；形状一变，老用户的文件就读不回来。
        let mut likes = Likes::default();
        likes.toggle(TrackId(1));
        likes.toggle(TrackId(999));

        let mut ids: Vec<&TrackId> = likes.ids.iter().collect();
        ids.sort_unstable();
        let text = serde_json::to_string(&ids).expect("序列化不该失败");
        assert_eq!(text, "[1,999]", "TrackId 应当序列化成裸数字数组");

        let parsed: Vec<TrackId> = serde_json::from_str(&text).expect("反序列化不该失败");
        let back = Likes {
            ids: parsed.into_iter().collect(),
        };
        assert_eq!(back, likes);
    }

    #[test]
    fn corrupt_file_yields_empty_set() {
        let parsed: Vec<TrackId> = serde_json::from_str("这不是 JSON").unwrap_or_default();
        assert!(parsed.is_empty());
    }

    #[test]
    fn insert_all_counts_only_the_new_ones() {
        let mut likes = Likes::default();
        likes.toggle(TrackId(1));

        assert_eq!(
            likes.insert_all([TrackId(1), TrackId(2)]),
            1,
            "重复的不应计数"
        );
        assert!(likes.contains(TrackId(2)));
        assert_eq!(likes.insert_all([]), 0, "空批量不该改动集合");
    }
}
