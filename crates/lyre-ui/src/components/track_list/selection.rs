//! 多选状态机：勾选、Shift 连选，以及四个批量动作的落地。

use std::collections::HashSet;
use std::rc::Rc;

use lyre_core::{CollectionId, TrackId};

use crate::entities::LibraryEntity;
use crate::platform::*;

use super::TrackList;

impl TrackList {
    pub(super) fn weak(cx: &Context<Self>) -> WeakEntity<Self> {
        cx.entity().downgrade()
    }

    /// 开关多选模式。关掉时顺手清掉勾选，下次打开从干净状态开始。
    pub(super) fn toggle_selection_mode(&mut self, cx: &mut Context<Self>) {
        self.selecting = !self.selecting;
        if self.selecting {
            cx.notify();
            return;
        }
        self.clear_chosen(cx);
    }

    /// 勾选 / 取消一行。按住 Shift 从上一次点中的行连选到这一行。
    pub(super) fn pick_row(
        &mut self,
        index: usize,
        track: TrackId,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        if shift {
            if let Some(anchor) = self.anchor {
                let ids = self
                    .library
                    .read(cx)
                    .visible_ids(anchor.min(index)..anchor.max(index) + 1);
                Rc::make_mut(&mut self.selected).extend(ids);
            }
        } else {
            let selected = Rc::make_mut(&mut self.selected);
            if !selected.remove(&track) {
                selected.insert(track);
            }
        }
        self.anchor = Some(index);
        cx.notify();
    }

    pub(super) fn choose_all_visible(&mut self, cx: &mut Context<Self>) {
        let count = self.library.read(cx).visible_len();
        let ids = self.library.read(cx).visible_ids(0..count);
        Rc::make_mut(&mut self.selected).extend(ids);
        cx.notify();
    }

    pub(super) fn clear_chosen(&mut self, cx: &mut Context<Self>) {
        Rc::make_mut(&mut self.selected).clear();
        self.anchor = None;
        cx.notify();
    }

    /// 可见投影换代后清掉已经看不见的选择。只在查询 / 作用域 / 删歌发生时调用，
    /// 渲染路径因此不必每帧扫全部可见行。
    pub(super) fn prune_selection(&mut self, cx: &mut App) {
        if self.selected.is_empty() {
            self.anchor = None;
            return;
        }
        let library = self.library.read(cx);
        let selected = Rc::make_mut(&mut self.selected);
        selected.retain(|id| library.is_visible(*id));
        if selected.is_empty() {
            self.anchor = None;
        }
    }

    /// 勾选中的曲目，按当前可见行顺序。只在批量动作真正执行或菜单展开时调用。
    pub(super) fn chosen(&mut self, cx: &mut App) -> Vec<TrackId> {
        if self.selected.is_empty() {
            return Vec::new();
        }
        let library = self.library.read(cx);
        let mut rows: Vec<(usize, TrackId)> = self
            .selected
            .iter()
            .filter_map(|id| library.row_of(*id).map(|row| (row, *id)))
            .collect();
        rows.sort_unstable_by_key(|(row, _)| *row);
        // 理论上投影换代时已经清理过；这里兜住漏网的 id，不让批量动作写到看不见的曲目。
        if rows.len() != self.selected.len() {
            self.selected = Rc::new(rows.iter().map(|(_, id)| *id).collect());
        }
        rows.into_iter().map(|(_, id)| id).collect()
    }

    pub(super) fn like_chosen(&mut self, cx: &mut Context<Self>) {
        let ids = self.chosen(cx);
        if ids.is_empty() {
            return;
        }
        self.library
            .update(cx, |library, cx| library.like_many(&ids, cx));
    }

    pub(super) fn add_chosen_to(&mut self, id: CollectionId, cx: &mut Context<Self>) {
        let ids = self.chosen(cx);
        if ids.is_empty() {
            return;
        }
        self.library.update(cx, |library, cx| {
            library.add_many_to_collection(id, &ids, cx);
        });
    }

    pub(super) fn new_collection_with_chosen(&mut self, cx: &mut Context<Self>) {
        let ids = self.chosen(cx);
        if ids.is_empty() {
            return;
        }
        self.library.update(cx, |library, cx| {
            // 名字带序号：连着建两个而不改名，侧栏会出现两行同名
            let name = format!("新歌单 {}", library.collections().len() + 1);
            if let Some(id) = library.create_collection(&name, cx) {
                library.add_many_to_collection(id, &ids, cx);
            }
        });
    }

    pub(super) fn remove_chosen_from(&mut self, id: CollectionId, cx: &mut Context<Self>) {
        let ids = self.chosen(cx);
        if ids.is_empty() {
            return;
        }
        self.library.update(cx, |library, cx| {
            library.remove_many_from_collection(id, &ids, cx);
        });
    }
}

/// 按可见行顺序给一组选择排序；批量菜单的 ✓ 判定要用有序 ID。
pub(super) fn ordered_chosen(library: &LibraryEntity, selected: &HashSet<TrackId>) -> Vec<TrackId> {
    ordered_by_row(selected, |id| library.row_of(id))
}

/// 只按已选集合查行号并排序，复杂度是 O(选中数 log 选中数)，与可见行数无关。
fn ordered_by_row(
    selected: &HashSet<TrackId>,
    mut row_of: impl FnMut(TrackId) -> Option<usize>,
) -> Vec<TrackId> {
    let mut rows: Vec<(usize, TrackId)> = selected
        .iter()
        .filter_map(|id| row_of(*id).map(|row| (row, *id)))
        .collect();
    rows.sort_unstable_by_key(|(row, _)| *row);
    rows.into_iter().map(|(_, id)| id).collect()
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`platform::*` 里的 gpui `test` 过程宏会遮蔽内置 `#[test]`。
    use std::collections::HashSet;

    use lyre_core::TrackId;

    use super::ordered_by_row;

    /// 10 万行里只选一首时，生成有序 ID 只该查这一次选择，而不是扫一遍可见行。
    #[test]
    fn ordering_one_selection_in_a_hundred_thousand_rows_is_constant_work() {
        let selected = HashSet::from([TrackId(99_999)]);
        let mut lookups = 0usize;

        let ordered = ordered_by_row(&selected, |id| {
            lookups += 1;
            (id == TrackId(99_999)).then_some(id.0 as usize)
        });

        assert_eq!(ordered, [TrackId(99_999)]);
        assert_eq!(lookups, 1, "有序 ID 的生成不该随可见行数增长");
    }

    #[test]
    fn ordering_follows_visible_rows_not_selection_order() {
        let selected = HashSet::from([TrackId(30), TrackId(10), TrackId(20)]);
        let rows = |id: TrackId| Some(id.0 as usize);

        assert_eq!(
            ordered_by_row(&selected, rows),
            [TrackId(10), TrackId(20), TrackId(30)]
        );
    }

    #[test]
    fn ids_that_left_the_projection_are_dropped() {
        let selected = HashSet::from([TrackId(1), TrackId(2)]);
        assert_eq!(
            ordered_by_row(&selected, |id| (id == TrackId(2)).then_some(0)),
            [TrackId(2)],
            "搜索筛掉的曲目不能留在批量动作的目标里"
        );
    }

    /// 性能回归：10 万行只选 1 首时，封面连续完成带来的每一次重绘都不该扫可见行。
    #[test]
    #[ignore = "性能基准，手动运行"]
    fn a_hundred_thousand_rows_with_one_selection_do_not_scan_per_repaint() {
        use std::hint::black_box;
        use std::time::Instant;

        const REPAINTS: usize = 1_000;
        let visible: Vec<TrackId> = (0..100_000).map(TrackId).collect();
        let selected = HashSet::from([TrackId(99_999)]);

        let old = Instant::now();
        for _ in 0..REPAINTS {
            // 旧路径：每帧复制全部可见 id 再过滤。
            let chosen: Vec<TrackId> = visible
                .iter()
                .copied()
                .filter(|id| selected.contains(id))
                .collect();
            black_box(chosen);
        }
        let old = old.elapsed();

        let new = Instant::now();
        for _ in 0..REPAINTS {
            // 新路径：投影没换代就只读一个计数。
            black_box(selected.len());
        }
        let new = new.elapsed();

        println!("10 万行 / 1 选中 / {REPAINTS} 次重绘：旧 {old:?}，新 {new:?}");
        assert!(
            new * 100 < old,
            "新渲染路径应当比逐帧全量过滤快两个数量级（{new:?} vs {old:?}）"
        );
    }
}
