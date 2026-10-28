//! 喜欢与关注：喜欢按 [`TrackId`] 键控，关注按艺术家名，都不随重新导入失效。
//! 各自独立落盘（`likes.json` / `follows.json`），`toggle_*` 返回切换后的值并立即写盘。

use lyre_core::TrackId;

use crate::platform::*;

use super::{LibraryEntity, LibraryEvent};

impl LibraryEntity {
    /// 这一首是否已收藏（`O(1)`）
    pub fn is_liked(&self, id: TrackId) -> bool {
        self.liked.contains(id)
    }

    pub fn liked_count(&self) -> usize {
        self.liked.len()
    }

    /// 切换收藏状态并返回切换后的值，每次立即写盘（攒批会丢退出前没存的那几次）；写失败静默。
    pub fn toggle_like(&mut self, id: TrackId, cx: &mut Context<Self>) -> bool {
        let now_liked = self.liked.toggle(id);
        self.liked.save();
        self.touch_data();
        cx.notify();
        now_liked
    }

    /// 这位艺术家是否已关注（`O(1)`）
    pub fn is_following(&self, name: &str) -> bool {
        self.followed.contains(name)
    }

    pub fn followed_count(&self) -> usize {
        self.followed.len()
    }

    /// 切换关注状态并返回切换后的值，每次立即写盘，写失败静默。
    pub fn toggle_follow(&mut self, name: impl AsRef<str>, cx: &mut Context<Self>) -> bool {
        let now_following = self.followed.toggle(name.as_ref());
        self.followed.save();
        self.touch_data();
        // 头图在 `RootView` 里内联渲染，只有事件能把它推重绘（见 [`LibraryEvent::FollowChanged`]）。
        cx.emit(LibraryEvent::FollowChanged);
        cx.notify();
        now_following
    }

    /// 批量收藏，只落盘一次。已是收藏的不重复计数。
    pub fn like_many(&mut self, tracks: &[TrackId], cx: &mut Context<Self>) -> usize {
        let added = self.liked.insert_all(tracks.iter().copied());
        if added == 0 {
            return 0;
        }
        self.liked.save();
        self.touch_data();
        cx.notify();
        added
    }

    /// 收藏的曲目，按曲库当前顺序。收藏没有自己的顺序 —— 它是个集合而不是列表。
    pub fn liked_tracks(&self) -> Vec<TrackId> {
        self.playlist
            .tracks()
            .iter()
            .filter(|track| self.liked.contains(track.id))
            .map(|track| track.id)
            .collect()
    }
}
