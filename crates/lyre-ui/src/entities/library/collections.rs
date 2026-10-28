//! 自建歌单：增删改、成员顺序、封面。成员是有序表，顺序由用户拖拽决定。
//! 对外报的成员数是当前曲库里还解析得出的那几首，不是 `Collection::len()`。

use std::fs;
use std::path::{Path, PathBuf};

use lyre_core::{CollectionId, Collections, TrackId};

use crate::platform::*;

use super::LibraryEntity;
use super::scope::Scope;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollectionSummary {
    /// 当前曲库里真的解析得出的成员数。不是 `Collection::len()` —— 后者会报幽灵行。
    pub count: usize,
    /// 用户为这个歌单指定的封面（数据目录里的那份拷贝）。
    pub cover_file: Option<PathBuf>,
    /// 第一个带内嵌封面的成员，取它当封面的代表。
    pub cover_id: Option<TrackId>,
}

/// 认得的图片扩展名，只决定拷贝时保留哪个后缀；能否解码由图片库判定。
const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// FNV-1a：只当文件名用，稳定即可。
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 「插到第 `boundary` 位之前」换算成 [`lyre_core::Collection::move_item`] 要的落点下标。
/// 往前挪时，被挪的那首空出来的位置会把后面的整体顶后一格，所以要减一；往后挪不用。
fn reorder_target(source: usize, boundary: usize) -> usize {
    if source < boundary {
        boundary - 1
    } else {
        boundary
    }
}

impl LibraryEntity {
    pub fn collections(&self) -> &Collections {
        &self.collections
    }

    /// 「我的歌单」那一排的顺序：收藏在最前，其余按创建顺序。
    pub fn list_scopes(&self) -> Vec<Scope> {
        let mut scopes = vec![Scope::Favorites];
        scopes.extend(
            self.collections
                .iter()
                .map(|list| Scope::Collection(list.id)),
        );
        scopes
    }

    /// 建一个歌单并返回它的 id。空名被拒（返回 `None`）。
    pub fn create_collection(
        &mut self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Option<CollectionId> {
        let list = self.collections.create(name)?;
        let id = list.id;
        self.persist_collections();
        self.touch_data();
        cx.notify();
        Some(id)
    }

    /// 删掉一个歌单。若正看着它就先退回整个曲库，否则面包屑会指向一个不存在的东西。
    pub fn delete_collection(&mut self, id: CollectionId, cx: &mut Context<Self>) -> bool {
        if !self.collections.remove(id) {
            return false;
        }
        if self.scope == Scope::Collection(id) {
            self.set_scope(Scope::All, cx);
        }
        self.recompute_visible(cx);
        self.persist_collections();
        self.touch_data();
        cx.notify();
        true
    }

    /// 重命名。空名与查不到 id 都返回 `false`。
    pub fn rename_collection(
        &mut self,
        id: CollectionId,
        name: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.collections.rename(id, name) {
            return false;
        }
        self.persist_collections();
        self.touch_data();
        // 正看着它时面包屑要立刻换名字
        cx.notify();
        true
    }

    /// 把一首加进歌单，返回是否真的新增（已在册不重复、也不挪位置）。
    pub fn add_to_collection(
        &mut self,
        id: CollectionId,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(list) = self.collections.get_mut(id) else {
            return false;
        };
        if !list.add(track) {
            return false;
        }
        self.persist_collections();
        self.touch_data();
        // 正看着这个歌单时新加的那首要立刻出现
        if self.scope == Scope::Collection(id) {
            self.recompute_visible(cx);
        }
        cx.notify();
        true
    }

    pub fn remove_from_collection(
        &mut self,
        id: CollectionId,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(list) = self.collections.get_mut(id) else {
            return false;
        };
        if !list.remove(track) {
            return false;
        }
        self.persist_collections();
        self.touch_data();
        if self.scope == Scope::Collection(id) {
            self.recompute_visible(cx);
        }
        cx.notify();
        true
    }

    /// 批量加入同一个歌单，只落一次盘，返回真正新增的条数。
    pub fn add_many_to_collection(
        &mut self,
        id: CollectionId,
        tracks: &[TrackId],
        cx: &mut Context<Self>,
    ) -> usize {
        let Some(list) = self.collections.get_mut(id) else {
            return 0;
        };
        let added = list.add_all(tracks.iter().copied());
        if added == 0 {
            return 0;
        }
        self.persist_collections();
        self.touch_data();
        if self.scope == Scope::Collection(id) {
            self.recompute_visible(cx);
        }
        cx.notify();
        added
    }

    pub fn remove_many_from_collection(
        &mut self,
        id: CollectionId,
        tracks: &[TrackId],
        cx: &mut Context<Self>,
    ) -> usize {
        let Some(list) = self.collections.get_mut(id) else {
            return 0;
        };
        let removed = tracks.iter().filter(|track| list.remove(**track)).count();
        if removed == 0 {
            return 0;
        }
        self.persist_collections();
        self.touch_data();
        if self.scope == Scope::Collection(id) {
            self.recompute_visible(cx);
        }
        cx.notify();
        removed
    }

    /// 拖拽排序：把可见行 `from` 插到第 `gap` 行之前（`gap` 可以是行数本身 = 末尾）。
    /// 只在歌单作用域里成立 —— 专辑 / 艺术家 / 收藏的顺序由曲库决定，拖动它们改不了任何东西。
    pub fn reorder_collection(&mut self, from: usize, gap: usize, cx: &mut Context<Self>) -> bool {
        let Scope::Collection(id) = self.scope else {
            return false;
        };
        let positions = self.collection_positions(id);
        let Some(&source) = positions.get(from) else {
            return false;
        };
        let boundary = match positions.get(gap) {
            Some(&position) => position,
            None => self
                .collections
                .get(id)
                .map_or(0, lyre_core::Collection::len),
        };
        let moved = match self.collections.get_mut(id) {
            Some(list) => list.move_item(source, reorder_target(source, boundary)),
            None => false,
        };
        if !moved {
            return false;
        }
        // 成员顺序就是行顺序，不重算投影的话界面还停在旧顺序上。
        self.recompute_visible(cx);
        self.persist_collections();
        self.touch_data();
        cx.notify();
        true
    }

    /// 可见行 → 歌单成员数组的下标。解析不出曲目的成员不占行，两个轴不是同一个，必须映射。
    fn collection_positions(&self, id: CollectionId) -> Vec<usize> {
        let Some(list) = self.collections.get(id) else {
            return Vec::new();
        };
        list.tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| self.playlist.by_id(**track).is_some())
            .map(|(position, _)| position)
            .collect()
    }

    /// 歌单的名字与成员（歌单顺序）。id 查不到返回 `None`，头图就不画。
    pub fn collection_members(&self, id: CollectionId) -> Option<(String, Vec<TrackId>)> {
        let list = self.collections.get(id)?;
        Some((list.name.clone(), list.tracks.clone()))
    }

    /// 某个歌单当前能解析出曲目的成员数。
    pub fn collection_len(&self, id: CollectionId) -> usize {
        self.collection_summary(id).count
    }

    pub fn collection_cover(&self, id: CollectionId) -> Option<PathBuf> {
        self.collections.get(id)?.cover.clone()
    }

    /// 侧栏一行歌单要的三样数据，一次成员扫描算完（拆成三个 getter 就是每帧扫三遍）。
    pub fn collection_summary(&self, id: CollectionId) -> CollectionSummary {
        let Some(list) = self.collections.get(id) else {
            return CollectionSummary::default();
        };
        let mut summary = CollectionSummary {
            cover_file: list.cover.clone(),
            ..Default::default()
        };
        for member in &list.tracks {
            let Some(track) = self.playlist.by_id(*member) else {
                continue;
            };
            summary.count += 1;
            if summary.cover_id.is_none() && track.has_cover {
                summary.cover_id = Some(*member);
            }
        }
        summary
    }

    /// 弹原生图片选择器换歌单封面。回调只把路径交给 [`Self::set_collection_cover`]，校验在那里做。
    pub fn pick_collection_cover(&mut self, id: CollectionId, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from("选择歌单封面图片")),
        });
        cx.spawn(async move |this, cx| {
            // 外层 Result 是「对话框能不能开」，内层是「用户选了什么」
            let Ok(result) = receiver.await else { return };
            let Ok(Some(paths)) = result else { return };
            let Some(source) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| this.set_collection_cover(id, &source, cx))
                .ok();
        })
        .detach();
    }

    /// 图片拷进数据目录再记路径，不引用原图。落名带内容哈希，缓存因此不需要失效逻辑。
    pub fn set_collection_cover(
        &mut self,
        id: CollectionId,
        source: &Path,
        cx: &mut Context<Self>,
    ) {
        let extension = source
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
            .map_or_else(|| ".img".to_owned(), |ext| format!(".{ext}"));

        let Ok(bytes) = fs::read(source) else {
            log::warn!("歌单封面读不出来：{}", source.display());
            return;
        };
        let target_dir = match crate::persist::data_dir() {
            Some(dir) => dir.join("covers"),
            None => return,
        };
        if let Err(err) = fs::create_dir_all(&target_dir) {
            log::warn!("歌单封面目录建不出来：{err}");
            return;
        }
        let target = target_dir.join(format!("{}-{}{extension}", id.0, fnv1a(&bytes)));
        if let Err(err) = fs::write(&target, &bytes) {
            log::warn!("歌单封面写不进去：{err}");
            return;
        }

        let previous = self.collections.get(id).and_then(|list| list.cover.clone());
        if !self.collections.set_cover(id, Some(target)) {
            return;
        }
        // 只有换成功才删旧的，且只删自己数据目录里的
        if let Some(old) = previous.filter(|old| old.starts_with(&target_dir)) {
            let _ = fs::remove_file(old);
        }
        self.persist_collections();
        self.touch_data();
        cx.notify();
    }

    fn persist_collections(&self) {
        crate::collections_store::save(self.collections.clone());
    }
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`crate::platform::*` 带进来的 gpui `test` 宏会遮蔽内置 `#[test]`
    use super::reorder_target;

    /// 落点换算：`boundary` 是「插到第几行之前」，`reorder_target` 出的是「挪完之后站在第几位」。
    #[test]
    fn forward_drag_closes_the_gap_it_opens() {
        // 四行 [a,b,c,d]，把 a 插到 c 之前（boundary=2）→ 期望 [b,a,c,d]，a 落在下标 1
        assert_eq!(reorder_target(0, 2), 1);
        // 插到最前面不需要修正
        assert_eq!(reorder_target(3, 0), 0);
        // 插到末尾（boundary = 行数）→ 落在最后一格
        assert_eq!(reorder_target(0, 4), 3);
        // 把 d 插到 b 之前 → [a,d,b,c]，d 落在下标 1
        assert_eq!(reorder_target(3, 1), 1);
    }

    #[test]
    fn a_drop_lands_next_to_the_row_you_aimed_at() {
        use lyre_core::{Collection, CollectionId, TrackId};

        fn apply(rows: &[u64], from: usize, boundary: usize) -> Vec<u64> {
            let mut list = Collection::new(CollectionId(0), "演示");
            for row in rows {
                list.add(TrackId(*row));
            }
            // 这几行都能解析成曲目，所以可见行号与成员下标是同一个轴
            list.move_item(from, reorder_target(from, boundary));
            list.tracks.iter().map(|track| track.0).collect()
        }

        assert_eq!(
            apply(&[1, 2, 3, 4], 0, 2),
            vec![2, 1, 3, 4],
            "把 1 插到 3 之前"
        );
        assert_eq!(
            apply(&[1, 2, 3, 4], 3, 1),
            vec![1, 4, 2, 3],
            "把 4 插到 2 之前"
        );
        assert_eq!(
            apply(&[1, 2, 3, 4], 0, 4),
            vec![2, 3, 4, 1],
            "瞄准最后一行的下半 = 排到末尾"
        );
        assert_eq!(
            apply(&[1, 2, 3, 4], 2, 2),
            vec![1, 2, 3, 4],
            "落在自己的上半 = 原地不动"
        );
        assert_eq!(
            apply(&[1, 2, 3, 4], 2, 3),
            vec![1, 2, 3, 4],
            "落在自己的下半 = 原地不动"
        );
    }
}
