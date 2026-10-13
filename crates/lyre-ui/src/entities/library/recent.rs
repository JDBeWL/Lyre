//! 「最近播放」与队列的「即将播放 / 已播放」是两套口径：[`LibraryEntity::recent_tracks`] 取自
//! `recents.json`、跨重启；[`LibraryEntity::just_played`] 是当前队列光标前的几段，随队列重建消失。

use lyre_core::{Track, TrackId};

use crate::platform::*;

use super::LibraryEntity;

impl LibraryEntity {
    /// 播放队列：从当前往后 `limit` 首，按实际播放顺序（`order`，随机模式下与 `tracks` 不同）。
    pub fn upcoming(&self, limit: usize) -> Vec<Track> {
        let position = self.playlist.cursor();
        self.playlist
            .order()
            .iter()
            .skip(position)
            .take(limit)
            .filter_map(|id| self.playlist.by_id(*id).cloned())
            .collect()
    }

    /// 播放队列前 `limit` 个的 id，口径同 [`Self::upcoming`]，但不 clone `Track`。
    pub fn upcoming_ids(&self, limit: usize) -> Vec<TrackId> {
        let position = self.playlist.cursor();
        self.playlist
            .order()
            .iter()
            .skip(position)
            .filter(|id| self.playlist.by_id(**id).is_some())
            .take(limit)
            .copied()
            .collect()
    }

    /// 当前队列里光标前面那几段，顺序由远及近。不是播放史，首页的「最近播放」走 [`Self::recent_tracks`]。
    pub fn just_played(&self, limit: usize) -> Vec<Track> {
        let position = self.playlist.cursor();
        let start = position.saturating_sub(limit);
        self.playlist
            .order()
            .iter()
            .skip(start)
            .take(position - start)
            .filter_map(|id| self.playlist.by_id(*id).cloned())
            .collect()
    }

    /// 记一笔「这首开始播了」，跨重启保留。由 root 在 `PlaybackEvent::TrackChanged` 上转发，自动续播也走它，不会漏记。
    pub fn record_played(&mut self, id: TrackId, cx: &mut Context<Self>) {
        let before = self.recents.clone();
        self.recents.record(id);
        // 重播当前这首时列表没变，不该为此重写一遍盘。
        if self.recents != before {
            self.recents.save();
            cx.notify();
        }
    }

    /// 首页要的「最近播放」：最近在前的曲目，跳过已不在曲库里的。多取 4 倍再截断，否则开头压着失效 id 时会少几张卡。
    pub fn recent_tracks(&self, limit: usize) -> Vec<Track> {
        self.recents
            .newest(limit * 4)
            .iter()
            .filter_map(|id| self.playlist.by_id(*id))
            .take(limit)
            .cloned()
            .collect()
    }

    /// 记过的条数，含已失效的 id：它反映 `recents.json` 里真有多少条，用 `recent_tracks(..).len()` 会少报。
    pub fn recent_count(&self) -> usize {
        self.recents.len()
    }
}
