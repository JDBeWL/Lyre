//! 多选状态机：勾选、Shift 连选，以及四个批量动作的落地。

use std::rc::Rc;

use lyre_core::{CollectionId, TrackId};

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

    /// 勾选中的曲目，按当前可见行顺序；顺手把已经看不见的（搜索筛掉的、被移除的）从状态里剪掉。
    /// 渲染与四个批量动作都走它。
    pub(super) fn chosen(&mut self, cx: &mut App) -> Vec<TrackId> {
        if self.selected.is_empty() {
            return Vec::new();
        }
        let count = self.library.read(cx).visible_len();
        let visible = self.library.read(cx).visible_ids(0..count);
        let chosen: Vec<TrackId> = visible
            .into_iter()
            .filter(|id| self.selected.contains(id))
            .collect();
        // `chosen` 是 `selected` 的子集，数量相同就说明没有一个被筛掉，不必重建成集合。
        if chosen.len() != self.selected.len() {
            self.selected = Rc::new(chosen.iter().copied().collect());
        }
        chosen
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
