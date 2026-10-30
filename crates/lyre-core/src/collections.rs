//! 用户自建歌单：一组有序的曲目引用。与播放队列 [`crate::Playlist`] 是两件事：后者回答
//! 「接下来按什么顺序播」，会随切歌、洗牌、排序重排；`Collection` 只回答「在不在、排第几」。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::track::TrackId;

/// 歌单标识，由 [`Collections::create`] 从单调计数器发号，删除不回收（复用 id 会让导航历史指错）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct CollectionId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub id: CollectionId,
    pub name: String,
    /// 成员顺序即显示顺序，所以是 `Vec`。存 id 不存曲目，标签更新与重导时歌单自动跟着变。
    pub tracks: Vec<TrackId>,
    /// 用户指定的封面，指向数据目录里的一份拷贝而不是原图。`None` 时界面画派生色块。
    #[serde(default)]
    pub cover: Option<PathBuf>,
}

impl Collection {
    pub fn new(id: CollectionId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            tracks: Vec::new(),
            cover: None,
        }
    }

    pub fn contains(&self, id: TrackId) -> bool {
        self.tracks.contains(&id)
    }

    /// 加入一首。已在册时返回 `false` 且不动位置 —— 重复加入不该把歌挪到末尾。
    pub fn add(&mut self, id: TrackId) -> bool {
        if self.contains(id) {
            return false;
        }
        self.tracks.push(id);
        true
    }

    pub fn remove(&mut self, id: TrackId) -> bool {
        let before = self.tracks.len();
        self.tracks.retain(|known| *known != id);
        self.tracks.len() != before
    }

    /// 批量加入，返回真正新增的条数。已在册的保持原位。
    pub fn add_all(&mut self, ids: impl IntoIterator<Item = TrackId>) -> usize {
        let before = self.tracks.len();
        for id in ids {
            self.add(id);
        }
        self.tracks.len() - before
    }

    /// 把第 `from` 首挪走，挪完之后它就站在第 `to` 位（`to` 越界时按末位算）。
    /// 越界、`from == to` 都不动顺序并返回 `false`。
    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= self.tracks.len() {
            return false;
        }
        let moved = self.tracks.remove(from);
        let to = to.min(self.tracks.len());
        self.tracks.insert(to, moved);
        true
    }

    /// 曲目已不在曲库时成员 id 留着无害：视图按 id 解析，解析不出就不画那一行，也不主动清理。
    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }
}

/// 全部歌单，落盘见 `lyre_ui::collections_store`。数组顺序即界面顺序（创建顺序）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Collections {
    /// 下一个可发的 id。只增不减。
    #[serde(default)]
    next: u64,
    #[serde(default)]
    items: Vec<Collection>,
}

impl Collections {
    /// 把发号器推到已有 id 之上。文件里的 `next` 比最大 id 还小（手改过、或旧版本写的）时，
    /// `create` 会发出重复 id，那两个歌单就会互相顶掉封面与删除。
    pub fn normalize_next(&mut self) {
        if let Some(max) = self.items.iter().map(|item| item.id.0).max() {
            self.next = self.next.max(max + 1);
        }
    }

    /// 名字只 trim 不查重：重名是允许的（按 id 定位，重名不会造成歧义），
    /// 但空名会让侧栏出现一个没法点的条目，所以拒掉。
    pub fn create(&mut self, name: &str) -> Option<Collection> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let collection = Collection::new(CollectionId(self.next), name);
        self.next += 1;
        self.items.push(collection.clone());
        Some(collection)
    }

    pub fn remove(&mut self, id: CollectionId) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.id != id);
        self.items.len() != before
    }

    /// 换封面。`None` 是「恢复默认」，同样要落盘，所以不区分「没改到」。
    pub fn set_cover(&mut self, id: CollectionId, cover: Option<PathBuf>) -> bool {
        match self.items.iter_mut().find(|item| item.id == id) {
            Some(item) => {
                item.cover = cover;
                true
            }
            None => false,
        }
    }

    /// 改名。空名同样拒掉，返回是否改成了。
    pub fn rename(&mut self, id: CollectionId, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        match self.items.iter_mut().find(|item| item.id == id) {
            Some(item) => {
                name.clone_into(&mut item.name);
                true
            }
            None => false,
        }
    }

    pub fn get(&self, id: CollectionId) -> Option<&Collection> {
        self.items.iter().find(|item| item.id == id)
    }

    pub fn get_mut(&mut self, id: CollectionId) -> Option<&mut Collection> {
        self.items.iter_mut().find(|item| item.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Collection> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{Collection, CollectionId, Collections};
    use crate::track::TrackId;

    /// 空名不该产生一个「点不动的条目」，正名要 trim 掉首尾空白。
    #[test]
    fn create_trims_and_rejects_blank_names() {
        let mut lists = Collections::default();
        assert!(lists.create("   ").is_none(), "纯空白应当被拒");
        let made = lists.create("  深夜写歌  ").expect("正名不该被拒");
        assert_eq!(made.name, "深夜写歌");
        assert_eq!(lists.len(), 1);
    }

    /// id 必须单调：删掉一个之后新建的不能顶用同一个 id，
    /// 否则导航历史里还没退出的旧条目会指向新歌单。
    #[test]
    fn ids_are_never_reused() {
        let mut lists = Collections::default();
        let first = lists.create("一").expect("建一").id;
        assert!(lists.remove(first));
        let second = lists.create("二").expect("建二").id;
        assert_ne!(first, second, "id 被回收了");
        assert!(lists.get(first).is_none());
    }

    /// 重名允许：定位靠 id，不靠名字。
    #[test]
    fn duplicate_names_are_allowed() {
        let mut lists = Collections::default();
        let a = lists.create("通勤").expect("建 a").id;
        let b = lists.create("通勤").expect("建 b").id;
        assert_ne!(a, b);
        assert_eq!(lists.len(), 2);
    }

    #[test]
    fn add_is_idempotent_and_keeps_position() {
        let (x, y, z) = (TrackId(1), TrackId(2), TrackId(3));
        let mut list = Collection::new(CollectionId(0), "通勤");

        assert!(list.add(x));
        assert!(list.add(y));
        assert!(!list.add(x), "重复加入应被拒");
        assert_eq!(list.tracks, vec![x, y], "重复加入不该把歌挪到末尾");

        assert!(list.add(z));
        assert!(list.remove(y));
        assert_eq!(list.tracks, vec![x, z]);
        assert!(!list.remove(y), "再删一次应当无事发生");
        assert!(!list.contains(y));
    }

    #[test]
    fn rename_targets_one_list_only() {
        let mut lists = Collections::default();
        let a = lists.create("一").expect("建 a").id;
        let b = lists.create("二").expect("建 b").id;

        assert!(lists.rename(a, "改名"));
        assert_eq!(lists.get(a).expect("a 还在").name, "改名");
        assert_eq!(lists.get(b).expect("b 还在").name, "二");
        assert!(!lists.rename(a, "  "), "空名不该把条目清空");
        assert!(!lists.rename(CollectionId(999), "不存在"));
    }

    /// 拖拽排序落到数据层就是这一条：`to` 是「挪完之后站在第几位」。
    /// 界面上的「插到某行前面」要换算成这个下标，换算规则由 UI 层测试覆盖。
    #[test]
    fn move_item_puts_the_track_at_the_target_index() {
        let ids = |list: &Collection| list.tracks.iter().map(|t| t.0).collect::<Vec<_>>();
        let mut list = Collection::new(CollectionId(0), "通勤");
        for n in 1..=4 {
            list.add(TrackId(n));
        }

        assert!(list.move_item(0, 2));
        assert_eq!(ids(&list), vec![2, 3, 1, 4], "1 应当站在第 3 位");

        assert!(list.move_item(3, 0));
        assert_eq!(ids(&list), vec![4, 2, 3, 1]);

        // 往后挪到底：越界的 `to` 按末位算，而不是 panic
        assert!(list.move_item(0, 9));
        assert_eq!(ids(&list), vec![2, 3, 1, 4]);

        assert!(!list.move_item(1, 1), "原地不动不该算改成了");
        assert!(!list.move_item(9, 0), "越界不该改顺序");
        assert_eq!(ids(&list), vec![2, 3, 1, 4]);
    }

    #[test]
    fn add_all_counts_only_the_new_ones() {
        let mut list = Collection::new(CollectionId(0), "通勤");
        list.add(TrackId(1));

        assert_eq!(
            list.add_all([TrackId(1), TrackId(2), TrackId(3)]),
            2,
            "已在册的不该被重复计数"
        );
        assert_eq!(list.tracks, vec![TrackId(1), TrackId(2), TrackId(3)]);
    }

    #[test]
    fn normalize_next_keeps_new_ids_unique() {
        let mut lists = Collections::default();
        assert_eq!(
            lists.create("甲").expect("非空名字该建出来").id,
            CollectionId(0)
        );
        assert_eq!(
            lists.create("乙").expect("非空名字该建出来").id,
            CollectionId(1)
        );

        lists.next = 0;
        lists.normalize_next();
        assert_eq!(
            lists.create("丙").expect("非空名字该建出来").id,
            CollectionId(2),
            "不能发出重复 id"
        );
    }
}
