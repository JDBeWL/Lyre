use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::track::{Track, TrackId};

/// 大小写不敏感比较，不分配（`to_lowercase()` 会让每次比较都新建 String）。
fn cmp_ci(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

/// 缺失字段排在前面。
fn compare_by(key: SortKey, a: &Track, b: &Track) -> Ordering {
    match key {
        SortKey::ImportOrder => Ordering::Equal,
        SortKey::Title => cmp_ci(&a.title, &b.title),
        SortKey::Artist => cmp_ci(
            a.artist.as_deref().unwrap_or(""),
            b.artist.as_deref().unwrap_or(""),
        ),
        SortKey::Album => cmp_ci(
            a.album.as_deref().unwrap_or(""),
            b.album.as_deref().unwrap_or(""),
        ),
        SortKey::Duration => a
            .duration
            .unwrap_or_default()
            .cmp(&b.duration.unwrap_or_default()),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayMode {
    #[default]
    Sequential,
    Shuffle,
    RepeatAll,
    RepeatOne,
}

impl PlayMode {
    pub const ALL: [PlayMode; 4] = [
        PlayMode::Sequential,
        PlayMode::Shuffle,
        PlayMode::RepeatAll,
        PlayMode::RepeatOne,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PlayMode::Sequential => "顺序播放",
            PlayMode::Shuffle => "随机播放",
            PlayMode::RepeatAll => "列表循环",
            PlayMode::RepeatOne => "单曲循环",
        }
    }

    pub fn next_in_cycle(self) -> Self {
        match self {
            PlayMode::Sequential => PlayMode::Shuffle,
            PlayMode::Shuffle => PlayMode::RepeatAll,
            PlayMode::RepeatAll => PlayMode::RepeatOne,
            PlayMode::RepeatOne => PlayMode::Sequential,
        }
    }

    pub fn is_shuffle(self) -> bool {
        matches!(self, PlayMode::Shuffle)
    }
}

/// 曲库排序键。排序重排曲库本体而不是只改投影：用户期待「下一曲」按新键走。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    #[default]
    ImportOrder,
    Title,
    Artist,
    Album,
    Duration,
}

impl SortKey {
    pub const ALL: [SortKey; 5] = [
        SortKey::ImportOrder,
        SortKey::Title,
        SortKey::Artist,
        SortKey::Album,
        SortKey::Duration,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::ImportOrder => "导入顺序",
            SortKey::Title => "标题",
            SortKey::Artist => "艺术家",
            SortKey::Album => "专辑",
            SortKey::Duration => "时长",
        }
    }
}

/// 队列推进的结论：只回答「该去哪」，不真的去播。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advance {
    /// 前进到播放顺序中的第 n 个位置
    To(usize),
    /// 回到当前曲目的开头
    Restart,
    /// 队列到头了，停下
    Stop,
}

/// 播放列表与队列。`order` 存 [`TrackId`] 而非数组下标，删曲时按 id 过滤即可。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Playlist {
    tracks: Vec<Track>,
    #[serde(skip)]
    index: HashMap<TrackId, usize>,
    /// 播放顺序（元素是曲目 id）
    order: Vec<TrackId>,
    /// 在 `order` 中的位置
    pos: usize,
    mode: PlayMode,
    /// 队列走到末尾要不要绕回开头。见 [`Self::set_wrap_at_end`]。
    #[serde(skip)]
    wrap_at_end: bool,
    /// 原始导入顺序，用于切回 `SortKey::ImportOrder`
    import_seq: Vec<TrackId>,
    /// 当前排序键，只用于界面上显示「现在按什么排的」。
    sort: SortKey,
    /// 播放队列：`None` = 整个曲库。有值时是「从某个列表开始播放」那批 id，按那个列表的显示顺序，
    /// 只决定 `order` 的成分，不动 `tracks`。
    #[serde(skip)]
    queue: Option<Vec<TrackId>>,
    /// 队列结构或游标的代次：队列面板据此缓存行模型，避免每次重绘都全量取曲目。
    #[serde(skip)]
    revision: u64,
}

impl Playlist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn mode(&self) -> PlayMode {
        self.mode
    }

    pub fn order(&self) -> &[TrackId] {
        &self.order
    }

    pub fn cursor(&self) -> usize {
        self.pos
    }

    /// 队列结构或游标的代次。只在会改变 `order` / `pos` 的操作后前进。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn by_id(&self, id: TrackId) -> Option<&Track> {
        self.index.get(&id).map(|i| &self.tracks[*i])
    }

    /// 按导入顺序给出曲目，落盘用它而不是 `tracks()`：排序是视图状态，混着用会回不去导入顺序。
    pub fn in_import_order(&self) -> impl Iterator<Item = &Track> {
        self.import_seq.iter().filter_map(|id| self.by_id(*id))
    }

    pub fn index_of(&self, id: TrackId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    /// 当前曲目在曲库中的下标（不是 order 里的位置）。
    pub fn current_index(&self) -> Option<usize> {
        let id = self.order.get(self.pos)?;
        self.index.get(id).copied()
    }

    pub fn current(&self) -> Option<&Track> {
        let id = self.order.get(self.pos)?;
        self.by_id(*id)
    }

    pub fn current_id(&self) -> Option<TrackId> {
        self.order.get(self.pos).copied()
    }

    /// 播放顺序中第 `n` 位对应的曲目。
    pub fn track_at_order(&self, n: usize) -> Option<&Track> {
        self.order.get(n).and_then(|id| self.by_id(*id))
    }

    /// 并入曲目，返回新增的数量。同 id（同路径）不重复入库，但会用刚读到的标签原地替换旧条目：
    /// 曲库落盘之后，「重新导入这个文件夹」就是刷新标签的唯一入口。替换不动位置。
    pub fn push(&mut self, incoming: impl IntoIterator<Item = Track>) -> usize {
        let mut added = 0;
        for track in incoming {
            let existing = self.index.get(&track.id).copied();
            if let Some(index) = existing {
                self.tracks[index] = track;
            } else {
                self.index.insert(track.id, self.tracks.len());
                // 队列已定时新导入的曲目不进队列：队列是「从某个列表开始播」时定下的那批
                if self.queue.is_none() {
                    self.order.push(track.id);
                }
                self.import_seq.push(track.id);
                self.tracks.push(track);
                added += 1;
            }
        }
        if added > 0 {
            self.touch();
        }
        if self.sort != SortKey::ImportOrder {
            // 否则新歌全堆在末尾、刷新过标签的旧歌还停在原位，列表看着就没排过序
            self.apply_sort();
        }
        added
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.index.clear();
        self.order.clear();
        self.import_seq.clear();
        self.queue = None;
        self.pos = 0;
        self.touch();
    }

    /// 重建 `index`。任何改动 `tracks` 次序的地方都要调它。
    fn reindex(&mut self) {
        self.index = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id, i))
            .collect();
    }

    pub fn sort_key(&self) -> SortKey {
        self.sort
    }

    /// 换键并立即重排 `tracks` 与 `order`。
    pub fn sort_by(&mut self, key: SortKey) {
        self.sort = key;
        self.apply_sort();
    }

    /// 随机模式不重洗（保留现存 id 的相对序，新 id 追加末尾）；`pos` 跟着曲目 id 而非下标走。
    fn apply_sort(&mut self) {
        if self.sort == SortKey::ImportOrder {
            // 临时表把 position 查找从 O(n²) 压到 O(n)
            let position: HashMap<TrackId, usize> = self
                .import_seq
                .iter()
                .enumerate()
                .map(|(i, id)| (*id, i))
                .collect();
            self.tracks
                .sort_by_key(|t| position.get(&t.id).copied().unwrap_or(usize::MAX));
        } else {
            let key = self.sort;
            self.tracks.sort_by(|a, b| compare_by(key, a, b));
        }

        self.reindex();
        let current = self.current_id();

        // 队列已定时排序只影响 `tracks`，队列的成分与相对序不动 —— 之后改排序不该重排正在听的东西。
        let wanted = self.queue_ids();
        self.order = if self.mode.is_shuffle() {
            let allowed: HashSet<TrackId> = wanted.iter().copied().collect();
            let mut seen: HashSet<TrackId> = HashSet::with_capacity(wanted.len());
            let mut kept: Vec<TrackId> = Vec::with_capacity(wanted.len());
            for id in &self.order {
                if allowed.contains(id) && seen.insert(*id) {
                    kept.push(*id);
                }
            }
            for id in wanted {
                if seen.insert(id) {
                    kept.push(id);
                }
            }
            kept
        } else {
            wanted
        };

        self.pos = current
            .and_then(|id| self.order.iter().position(|x| *x == id))
            .unwrap_or(0);
        self.touch();
    }

    pub fn play_order_index(&mut self, n: usize) -> bool {
        if n < self.order.len() {
            if self.pos != n {
                self.pos = n;
                self.touch();
            }
            true
        } else {
            false
        }
    }

    /// 直接跳到某一首（按 id）。返回是否找到。
    pub fn jump_to(&mut self, id: TrackId) -> bool {
        match self.order.iter().position(|x| *x == id) {
            Some(n) => {
                if self.pos != n {
                    self.pos = n;
                    self.touch();
                }
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, id: TrackId) -> bool {
        let Some(idx) = self.index.remove(&id) else {
            return false;
        };
        self.tracks.remove(idx);
        self.import_seq.retain(|x| *x != id);

        self.reindex();

        if let Some(p) = self.order.iter().position(|x| *x == id) {
            self.order.remove(p);
            if p < self.pos {
                self.pos -= 1;
            }
        }
        if let Some(queue) = &mut self.queue {
            queue.retain(|x| *x != id);
        }
        if self.order.is_empty() {
            self.pos = 0;
        } else {
            self.pos = self.pos.min(self.order.len() - 1);
        }
        self.touch();
        true
    }

    /// 撤销移除。同时插入 `tracks` 与 `order`（顺序模式的不变量是两者同序），
    /// 已存在同 id 返回 `false`。随机模式下只能大致还原到列表原处。
    pub fn insert_at(&mut self, track: Track, index: usize) -> bool {
        if self.index.contains_key(&track.id) {
            return false;
        }

        let track_index = index.min(self.tracks.len());
        let order_index = index.min(self.order.len());

        self.tracks.insert(track_index, track);
        self.reindex();

        let id = self.tracks[track_index].id;
        // 否则「撤销移除 → 切回导入顺序」会把它排到最后
        self.import_seq
            .insert(track_index.min(self.import_seq.len()), id);
        // 队列已定时不进 `order`：队列是「从某个列表开始播」时定下的那批，回插一首不该
        // 插进正在听的队列（曲库视图里队列是 `None`，撤销移除照旧回到播放顺序里）。
        if self.queue.is_none() {
            self.order.insert(order_index, id);
            // 插在光标前要把光标顶后一格，否则它指向另一首
            if order_index < self.pos {
                self.pos += 1;
            }
        }
        if !self.order.is_empty() {
            self.pos = self.pos.min(self.order.len() - 1);
        }
        self.touch();
        true
    }

    /// 整批回插，撤销「清空」用。追加语义：撤销时曲库本来就是空的；重复 id 跳过并保持原位。
    pub fn restore_all(&mut self, incoming: Vec<Track>) -> usize {
        let mut restored = 0usize;
        for track in incoming {
            let id = track.id;
            if self.index.contains_key(&id) {
                continue;
            }
            self.tracks.push(track);
            self.index.insert(id, self.tracks.len() - 1);
            self.import_seq.push(id);
            if self.queue.is_none() {
                self.order.push(id);
            }
            restored += 1;
        }
        if !self.order.is_empty() {
            self.pos = self.pos.min(self.order.len() - 1);
        }
        if restored > 0 {
            self.touch();
        }
        restored
    }

    /// 列表循环这条轴单独传：四态枚举装不下「随机 + 列表循环」，折成 `Shuffle` 后走到末尾只会停。
    pub fn set_wrap_at_end(&mut self, wrap: bool) {
        self.wrap_at_end = wrap;
    }

    pub fn set_mode(&mut self, mode: PlayMode, seed: u64) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.rebuild_order(seed);
    }

    /// 把播放队列定为这批曲目（按给定顺序）；`None` 恢复成整个曲库。
    /// 不存在的 id 丢掉，重复的只留第一次 —— 队列可以由界面直接把可见行喂进来。
    pub fn set_queue(&mut self, ids: Option<Vec<TrackId>>, seed: u64) {
        self.queue = ids.map(|ids| {
            let mut already_seen = HashSet::with_capacity(ids.len());
            ids.into_iter()
                .filter(|id| self.index.contains_key(id) && already_seen.insert(*id))
                .collect()
        });
        self.rebuild_order(seed);
    }

    /// 当前播放队列；`None` 表示「跟着曲库走」。
    pub fn queue(&self) -> Option<&[TrackId]> {
        self.queue.as_deref()
    }

    /// `order` 由哪些曲目组成：有队列用队列，否则整库。队列里的 id 仍要按 `index` 再过滤一次。
    fn queue_ids(&self) -> Vec<TrackId> {
        match &self.queue {
            Some(ids) => ids
                .iter()
                .copied()
                .filter(|id| self.index.contains_key(id))
                .collect(),
            None => self.tracks.iter().map(|t| t.id).collect(),
        }
    }

    /// 随机模式把当前曲目旋转到随机序首位，否则排在它前面的曲目这一轮永远走不到。
    pub fn rebuild_order(&mut self, seed: u64) {
        let current = self.current_id();

        self.order = self.queue_ids();

        if self.mode.is_shuffle() {
            shuffle(&mut self.order, seed);
            if let Some(id) = current
                && let Some(at) = self.order.iter().position(|x| *x == id)
            {
                self.order.rotate_left(at);
            }
            self.pos = 0;
        } else {
            // 光标跟着当前曲目走，切模式不打断正在播的歌
            self.pos = current
                .and_then(|id| self.order.iter().position(|x| *x == id))
                .unwrap_or(0);
        }
        self.touch();
    }

    /// 用户点了「下一曲」。播不出来的曲目照常留在曲库与列表里（界面置灰），队列不在其上停留 ——
    /// 否则「下一曲」只会换来一次「格式不支持」的报错。整队都播不了时按走到头处理。
    #[allow(clippy::should_implement_trait)] // 不是迭代器：它会按循环档位绕回开头，也会在末尾停下
    pub fn next(&mut self) -> Advance {
        if self.order.is_empty() {
            return Advance::Stop;
        }
        let wrap = self.mode == PlayMode::RepeatAll || self.wrap_at_end;
        let mut n = self.pos;

        // 最多走一圈：走满还没碰到能播的，就是整队都播不了
        for _ in 0..self.order.len() {
            if n + 1 < self.order.len() {
                n += 1;
            } else if wrap {
                n = 0;
            } else {
                return Advance::Stop;
            }

            if self.playable_at(n) {
                if self.pos != n {
                    self.pos = n;
                    self.touch();
                }
                return Advance::To(n);
            }
        }

        Advance::Stop
    }

    /// 已在首位时重播当前曲而非绕到末尾。同样跳过播不了的曲目；
    /// 光标之前一首能播的都没有时，退回「重播当前」。
    pub fn prev(&mut self) -> Advance {
        if self.order.is_empty() {
            return Advance::Stop;
        }
        let wrap = self.mode == PlayMode::RepeatAll || self.wrap_at_end;
        let mut n = self.pos;

        for _ in 0..self.order.len() {
            if n > 0 {
                n -= 1;
            } else if wrap {
                n = self.order.len() - 1;
            } else {
                return Advance::Restart;
            }

            if self.playable_at(n) {
                if self.pos != n {
                    self.pos = n;
                    self.touch();
                }
                return Advance::To(n);
            }
        }

        Advance::Restart
    }

    /// 播放顺序第 `n` 位能不能播。位置越界（理论上不该发生）当作能播 ——
    /// 一个悬空下标不该把整条队列卡死。
    fn playable_at(&self, n: usize) -> bool {
        self.track_at_order(n).is_none_or(Track::is_playable)
    }

    /// 一首歌自然播完：与 [`Playlist::next`] 的区别是单曲循环只在此时回头重播。
    pub fn on_track_ended(&mut self) -> Advance {
        if self.mode == PlayMode::RepeatOne && !self.order.is_empty() {
            Advance::Restart
        } else {
            self.next()
        }
    }
}

fn shuffle(ids: &mut [TrackId], seed: u64) {
    // xorshift64：不要密码学强度，但同 seed 必须同结果
    let mut state = seed | 1; // 状态不能为 0
    for i in (1..ids.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        ids.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn track_ext(n: usize, ext: &str) -> Track {
        let path = PathBuf::from(format!("/music/{n}.{ext}"));
        let mut t = Track::fallback(&path);
        t.title = format!("Track {n}");
        t
    }

    fn track(n: usize) -> Track {
        track_ext(n, "flac")
    }

    /// 引擎放不出来的一首：`.wma` 没有解码实现。
    fn unplayable(n: usize) -> Track {
        track_ext(n, "wma")
    }

    fn playlist(n: usize) -> Playlist {
        let mut p = Playlist::new();
        p.push((0..n).map(track));
        p
    }

    fn titled(n: usize, title: &str) -> Track {
        let mut t = track(n);
        t.title = title.to_owned();
        t
    }

    fn titles(p: &Playlist) -> Vec<String> {
        p.tracks().iter().map(|t| t.title.clone()).collect()
    }

    #[test]
    fn sorting_by_title_reorders_both_display_and_playback() {
        let mut p = Playlist::new();
        p.push([titled(0, "charlie"), titled(1, "alpha"), titled(2, "bravo")]);

        p.sort_by(SortKey::Title);

        assert_eq!(titles(&p), ["alpha", "bravo", "charlie"]);
        assert_eq!(p.track_at_order(0).unwrap().title, "alpha");
        assert_eq!(p.track_at_order(2).unwrap().title, "charlie");
    }

    /// 重导只刷新标签（`added == 0`）时也要重排，否则改过名的歌还停在旧位置。
    #[test]
    fn reimport_that_only_refreshes_tags_still_resorts() {
        let mut p = Playlist::new();
        p.push([titled(0, "aaa"), titled(1, "bbb")]);
        p.sort_by(SortKey::Title);

        assert_eq!(p.push([titled(0, "zzz")]), 0, "同 id 是替换，不算新增");
        assert_eq!(titles(&p), ["bbb", "zzz"], "刷新过标签后仍要符合排序键");
    }

    #[test]
    fn sorting_is_case_insensitive_and_stable() {
        let mut p = Playlist::new();
        p.push([
            titled(0, "Beta"),
            titled(1, "alpha"),
            titled(2, "ALPHA"),
            titled(3, "beta"),
        ]);

        p.sort_by(SortKey::Title);

        assert_eq!(titles(&p), ["alpha", "ALPHA", "Beta", "beta"]);
    }

    #[test]
    fn sorting_does_not_interrupt_the_playing_track() {
        let mut p = Playlist::new();
        p.push([
            titled(0, "delta"),
            titled(1, "charlie"),
            titled(2, "bravo"),
            titled(3, "alpha"),
        ]);
        p.play_order_index(2);
        let playing = p.current_id().unwrap();
        let playing_title = p.current().unwrap().title.clone();

        p.sort_by(SortKey::Title);

        assert_eq!(p.current_id(), Some(playing), "排序不该把正在播的歌换掉");
        assert_eq!(p.current().unwrap().title, playing_title);
    }

    #[test]
    fn sorting_restores_import_order() {
        let mut p = Playlist::new();
        p.push([titled(0, "charlie"), titled(1, "alpha"), titled(2, "bravo")]);
        let original = titles(&p);

        p.sort_by(SortKey::Title);
        assert_ne!(titles(&p), original, "先确认排序确实生效了");

        p.sort_by(SortKey::ImportOrder);
        assert_eq!(titles(&p), original, "应当能回到导入顺序");
    }

    #[test]
    fn sorting_does_not_reshuffle_a_shuffled_queue() {
        let mut p = playlist(6);
        p.set_mode(PlayMode::Shuffle, 0xC0FF_EE00);
        let shuffled: Vec<TrackId> = p.order().to_vec();

        p.sort_by(SortKey::Title);

        // 比的是相对次序而非绝对位置：曲库次序变了，新出现的 id 会追加到末尾
        let after: Vec<TrackId> = p.order().to_vec();
        let relative = |ids: &[TrackId]| {
            let mut v: Vec<TrackId> = ids.to_vec();
            v.retain(|id| shuffled.contains(id));
            v
        };
        assert_eq!(
            relative(&after),
            relative(&shuffled),
            "排序不该改变已洗好的相对次序"
        );
    }

    #[test]
    fn sorting_by_duration_puts_unknown_lengths_first() {
        let mut p = Playlist::new();
        let mut short = track(0);
        short.duration = Some(std::time::Duration::from_secs(60));
        let mut long = track(1);
        long.duration = Some(std::time::Duration::from_secs(600));
        let unknown = track(2);

        p.push([short, long, unknown]);
        p.sort_by(SortKey::Duration);

        assert_eq!(p.tracks()[0].duration, None);
        assert_eq!(
            p.tracks()[1].duration,
            Some(std::time::Duration::from_secs(60))
        );
        assert_eq!(
            p.tracks()[2].duration,
            Some(std::time::Duration::from_secs(600))
        );
    }

    #[test]
    fn importing_while_sorted_places_new_tracks_in_order() {
        let mut p = Playlist::new();
        p.push([titled(0, "bravo"), titled(1, "delta")]);
        p.sort_by(SortKey::Title);

        p.push([titled(2, "alpha"), titled(3, "charlie")]);

        assert_eq!(
            titles(&p),
            ["alpha", "bravo", "charlie", "delta"],
            "排序状态下导入的新曲目也要落到正确位置"
        );
    }

    #[test]
    fn sorting_an_empty_or_single_track_playlist_is_harmless() {
        let mut empty = Playlist::new();
        empty.sort_by(SortKey::Title);
        assert!(empty.is_empty());

        let mut single = playlist(1);
        single.sort_by(SortKey::Album);
        assert_eq!(single.len(), 1);
        assert_eq!(single.current_id(), Some(track(0).id));
    }

    #[test]
    fn sort_key_labels_are_all_distinct() {
        let labels: Vec<&str> = SortKey::ALL.iter().map(|k| k.label()).collect();
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(labels.len(), unique.len(), "排序键的标签必须互不相同");
    }

    #[test]
    fn sequential_stops_at_tail() {
        let mut p = playlist(3);
        assert_eq!(p.next(), Advance::To(1));
        assert_eq!(p.next(), Advance::To(2));
        assert_eq!(p.next(), Advance::Stop, "顺序模式走到头应当停下");
        assert_eq!(p.cursor(), 2, "停下时光标不应越界");
    }

    /// 播不了的曲目照常留在曲库里，但队列不在它上面停留。
    #[test]
    fn queue_skips_tracks_the_engine_cannot_decode() {
        let mut p = Playlist::new();
        p.push([track(0), unplayable(1), track(2)]);

        assert_eq!(p.len(), 3, "播不了的也要入库，只是不能播");
        assert_eq!(p.next(), Advance::To(2), "播不了的那首应当被跳过");
        assert_eq!(p.current().unwrap().id, track(2).id);

        p.play_order_index(2);
        assert_eq!(p.prev(), Advance::To(0), "往回走同样要跳过它");
        assert_eq!(p.current().unwrap().id, track(0).id);
    }

    #[test]
    fn consecutive_unplayable_tracks_are_all_skipped() {
        let mut p = Playlist::new();
        p.push([
            track(0),
            unplayable(1),
            unplayable(2),
            unplayable(3),
            track(4),
        ]);

        assert_eq!(
            p.next(),
            Advance::To(4),
            "连着三首都播不了应当一路跳到能播的"
        );
        assert_eq!(p.current().unwrap().id, track(4).id);
    }

    /// 整队都播不了不能原地打转，也不能假装播得了 —— 一律当成走到头。
    #[test]
    fn an_entirely_unplayable_queue_stops_instead_of_spinning() {
        let mut p = Playlist::new();
        p.push([unplayable(0), unplayable(1), unplayable(2)]);

        assert_eq!(p.next(), Advance::Stop, "顺序模式");
        assert_eq!(p.prev(), Advance::Restart, "往回走按既有约定重播当前");

        // 循环档位也不能让它绕圈：绕一圈还是全播不了
        p.set_mode(PlayMode::RepeatAll, 1);
        assert_eq!(p.next(), Advance::Stop, "列表循环下同样应当停下");
        p.set_mode(PlayMode::RepeatOne, 1);
        assert_eq!(p.next(), Advance::Stop, "单曲循环下手动切歌同样应当停下");
    }

    /// 循环模式下绕一圈能回到当前这首时，就该继续播它 —— 它是队列里唯一能播的。
    #[test]
    fn wrapping_back_to_the_only_playable_track_keeps_playing() {
        let mut p = Playlist::new();
        p.push([track(0), unplayable(1), unplayable(2)]);
        p.set_mode(PlayMode::RepeatAll, 1);

        assert_eq!(p.next(), Advance::To(0), "绕一圈只有当前这首能播，就还播它");
        assert_eq!(p.current().unwrap().id, track(0).id);
    }

    /// 自然播完那一拍走的是同一条路，不然一首 ape 放完就会卡在它上面。
    #[test]
    fn finishing_a_track_also_skips_unplayable_ones() {
        let mut p = Playlist::new();
        p.push([track(0), unplayable(1), track(2)]);

        assert_eq!(p.on_track_ended(), Advance::To(2));
        assert_eq!(p.current().unwrap().id, track(2).id);

        // 单曲循环只在「播完的是能播的那首」时才原地重播
        p.set_mode(PlayMode::RepeatOne, 1);
        assert_eq!(p.on_track_ended(), Advance::Restart);
    }

    /// 队列已定时跳过同样只在队列内部生效，不会滑到队列外的能播曲目上去。
    #[test]
    fn skipping_never_escapes_a_pinned_queue() {
        let mut p = Playlist::new();
        // `set_queue` 会丢掉库里没有的 id，队列成员必须先真的在曲库里
        p.push([track(0), unplayable(1), track(2)]);
        p.set_queue(Some(vec![track(0).id, unplayable(1).id]), 7);
        assert_eq!(p.order().len(), 2, "先确认队列真的有两首");

        assert_eq!(
            p.next(),
            Advance::Stop,
            "队列里只剩一首播不了的，就该停，不能滑进整库那首能播的"
        );
    }

    /// 光标落在播不了的曲目上时（比如刚导入、`pos` 还在 0），下一曲仍要一路找到能播的那首。
    #[test]
    fn a_cursor_sitting_on_an_unplayable_track_can_still_move_on() {
        let mut p = Playlist::new();
        p.push([unplayable(0), unplayable(1), track(2)]);

        assert_eq!(
            p.current().unwrap().id,
            unplayable(0).id,
            "起始光标就在它上面"
        );
        assert_eq!(
            p.next(),
            Advance::To(2),
            "从它出发也要越过播不了的找到能播的"
        );
        assert_eq!(p.current().unwrap().id, track(2).id);
    }

    #[test]
    fn repeat_all_wraps_to_head() {
        let mut p = playlist(3);
        p.mode = PlayMode::RepeatAll;
        for _ in 0..2 {
            p.next();
        }
        assert_eq!(p.next(), Advance::To(0), "列表循环应当绕回开头");
    }

    #[test]
    fn repeat_one_restarts_on_end_but_next_still_advances() {
        let mut p = playlist(3);
        p.mode = PlayMode::RepeatOne;

        assert_eq!(p.on_track_ended(), Advance::Restart);
        assert_eq!(p.cursor(), 0, "自然结束应当原地重播");

        assert_eq!(p.next(), Advance::To(1), "手动切歌应当照常前进");
    }

    #[test]
    fn prev_at_head_restarts_then_wraps_in_repeat_all() {
        let mut p = playlist(3);
        assert_eq!(p.prev(), Advance::Restart, "首位再上一曲 → 重播当前");

        p.mode = PlayMode::RepeatAll;
        assert_eq!(p.prev(), Advance::To(2), "列表循环下应当绕到末尾");
    }

    /// 「随机 + 列表循环」被折成 `Shuffle`，循环那条轴必须另外传进来，否则走完一轮就停。
    #[test]
    fn shuffled_order_wraps_when_the_loop_axis_is_on() {
        let mut p = playlist(4);
        p.set_mode(PlayMode::Shuffle, 0xC0_FFEE);
        p.set_wrap_at_end(true);
        p.play_order_index(3);

        assert_eq!(p.next(), Advance::To(0), "随机序走完一轮应当绕回开头");
        assert_eq!(p.prev(), Advance::To(3), "绕回后按上一曲该回到末尾");

        p.set_wrap_at_end(false);
        p.play_order_index(3);
        assert_eq!(p.next(), Advance::Stop, "关掉循环轴就还是走完即停");
    }

    #[test]
    fn shuffle_exhausts_queue_before_repeating() {
        let mut p = playlist(20);
        p.set_mode(PlayMode::Shuffle, 0xDEAD_BEEF);

        let mut seen = std::collections::HashSet::new();
        seen.insert(p.current_id().unwrap());

        while let Advance::To(_) = p.next() {
            let id = p.current_id().unwrap();
            assert!(seen.insert(id), "随机序在走完一轮之前不应重复出现同一首");
        }

        assert_eq!(seen.len(), 20, "随机播放必须走遍全部曲目才允许重复");
        assert_eq!(p.cursor(), 19, "走完之后停在队列末尾，下一次才是 Stop");
    }

    #[test]
    fn enabling_shuffle_keeps_current_at_head_so_nothing_is_skipped() {
        let mut p = playlist(20);
        p.play_order_index(5);
        let playing = p.current_id().unwrap();

        p.set_mode(PlayMode::Shuffle, 0x1234_5678);

        assert_eq!(
            p.current_id(),
            Some(playing),
            "切到随机模式不应打断正在播放的曲目"
        );
        assert_eq!(p.cursor(), 0, "当前曲目应当被旋转到随机序首位");

        let mut seen = std::collections::HashSet::from([playing]);
        while let Advance::To(_) = p.next() {
            seen.insert(p.current_id().unwrap());
        }
        assert_eq!(seen.len(), 20, "随机序不应漏掉任何一首");
    }

    #[test]
    fn leaving_shuffle_restores_natural_order_around_current() {
        let mut p = playlist(10);
        p.set_mode(PlayMode::Shuffle, 99);
        let playing = p.current_id().unwrap();

        p.set_mode(PlayMode::Sequential, 100);

        assert_eq!(p.current_id(), Some(playing));
        let natural: Vec<TrackId> = p.tracks().iter().map(|t| t.id).collect();
        assert_eq!(p.order(), natural.as_slice(), "顺序模式应当还原为自然序");
    }

    #[test]
    fn shuffle_is_deterministic_for_a_seed() {
        let mut a = playlist(30);
        let mut b = playlist(30);
        a.set_mode(PlayMode::Shuffle, 42);
        b.set_mode(PlayMode::Shuffle, 42);
        assert_eq!(a.order(), b.order(), "同 seed 必须得到同一洗牌结果");
    }

    #[test]
    fn toggling_shuffle_keeps_current_track_playing() {
        let mut p = playlist(10);
        p.play_order_index(4);
        let playing = p.current_id().unwrap();

        p.set_mode(PlayMode::Shuffle, 7);

        assert_eq!(
            p.current_id(),
            Some(playing),
            "切到随机模式不应打断正在播放的曲目"
        );
    }

    #[test]
    fn push_dedupes_by_path() {
        let mut p = Playlist::new();
        assert_eq!(p.push(vec![track(1), track(2)]), 2);
        assert_eq!(p.push(vec![track(1), track(3)]), 1, "重复路径应被跳过");
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn removing_track_before_cursor_keeps_same_song() {
        let mut p = playlist(5);
        p.play_order_index(3);
        let playing = p.current_id().unwrap();

        let victim = p.track_at_order(1).unwrap().id;
        assert!(p.remove(victim));

        assert_eq!(p.current_id(), Some(playing), "删除前置曲目不应改变当前曲");
        assert_eq!(p.len(), 4);
        assert_eq!(p.cursor(), 2);
    }

    #[test]
    fn removing_current_track_falls_through_to_next() {
        let mut p = playlist(5);
        p.play_order_index(2);
        let victim = p.current_id().unwrap();
        let expected = p.track_at_order(3).unwrap().id;

        assert!(p.remove(victim));
        assert_eq!(p.current_id(), Some(expected), "删掉当前曲应当顺延到下一首");
    }

    #[test]
    fn removing_last_track_clamps_cursor() {
        let mut p = playlist(3);
        p.play_order_index(2);
        let victim = p.current_id().unwrap();
        assert!(p.remove(victim));
        assert_eq!(p.cursor(), 1, "删掉末尾曲目后光标需要回收");
    }

    #[test]
    fn empty_playlist_decisions_are_stop() {
        let mut p = Playlist::new();
        assert_eq!(p.next(), Advance::Stop);
        assert_eq!(p.prev(), Advance::Stop);
        assert_eq!(p.on_track_ended(), Advance::Stop);
        assert!(p.current().is_none());
    }

    #[test]
    fn undo_of_removal_restores_order_and_cursor() {
        let mut p = playlist(5);
        p.play_order_index(3);
        let playing = p.current_id().unwrap();

        let victim = p.track_at_order(1).unwrap().clone();
        assert!(p.remove(victim.id));

        assert!(p.insert_at(victim.clone(), 1));

        let ids: Vec<TrackId> = p.tracks().iter().map(|t| t.id).collect();
        let expected: Vec<TrackId> = (0..5).map(track).map(|t| t.id).collect();
        assert_eq!(ids, expected, "撤销后曲库顺序应当与删除前完全一致");
        assert_eq!(p.current_id(), Some(playing), "撤销不该把正在播放的歌换掉");
    }

    #[test]
    fn inserting_before_the_cursor_pushes_it_back() {
        let mut p = playlist(3);
        p.play_order_index(2);
        let playing = p.current_id().unwrap();

        assert!(p.insert_at(track(99), 0));

        assert_eq!(p.current_id(), Some(playing), "光标必须仍然指向原来那首");
        assert_eq!(p.cursor(), 3, "光标在播放序里的位置应当被顶后一格");
    }

    #[test]
    fn inserting_a_duplicate_is_rejected() {
        let mut p = playlist(3);
        let existing = p.track_at_order(1).unwrap().clone();
        assert!(!p.insert_at(existing, 0), "同一首曲目不该被重复插入");
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn insert_clamps_out_of_range_index() {
        let mut p = playlist(2);
        assert!(p.insert_at(track(42), 999));
        assert_eq!(p.len(), 3);
        assert_eq!(
            p.tracks().last().unwrap().id,
            track(42).id,
            "越界下标应当夹到末尾"
        );
    }

    #[test]
    fn emptying_then_restoring_a_playlist_keeps_every_track() {
        let mut p = playlist(4);
        let snapshot: Vec<Track> = p.tracks().to_vec();
        p.clear();
        assert!(p.is_empty());

        assert_eq!(p.restore_all(snapshot), 4, "整批回插不该丢掉任何一首");
        assert_eq!(p.len(), 4);
        let expected: Vec<TrackId> = (0..4).map(track).map(|t| t.id).collect();
        assert_eq!(
            p.tracks().iter().map(|t| t.id).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            p.order(),
            expected.as_slice(),
            "播放序也要跟着还原，否则撤销后「下一曲」是空的"
        );
        assert_eq!(
            p.in_import_order().map(|t| t.id).collect::<Vec<_>>(),
            expected,
            "导入序同理"
        );
        assert_eq!(
            p.by_id(track(2).id).map(|t| t.title.clone()),
            Some(track(2).title),
            "索引必须当场可用，不能等到下次重建"
        );
    }

    /// 清空后用户又导入过同一批文件，撤销时不能把这些 id 插成双份。
    #[test]
    fn restoring_skips_ids_that_are_already_back() {
        let mut p = playlist(3);
        let snapshot: Vec<Track> = p.tracks().to_vec();
        p.clear();
        p.push(vec![snapshot[1].clone()]);

        assert_eq!(p.restore_all(snapshot), 2, "已经回来的那首不该再插一遍");
        assert_eq!(p.len(), 3);
        let ids: Vec<TrackId> = p.tracks().iter().map(|t| t.id).collect();
        let distinct: HashSet<TrackId> = ids.iter().copied().collect();
        assert_eq!(distinct.len(), 3, "不该出现重复 id");
        assert_eq!(p.order().len(), 3);
    }

    /// 队列光标在 push 完就非空，它只回答「从哪开始播」，不代表正在播放，不能当界面状态用。
    #[test]
    fn queue_cursor_points_at_the_first_track_before_anything_plays() {
        let p = playlist(3);
        assert_eq!(
            p.current_id(),
            Some(track(0).id),
            "队列光标在导入完成时就非空 —— 这正是它不能当「正在播放」用的原因"
        );
    }

    /// 队列面板按代次缓存行模型：只有结构 / 游标变化才前进，刷新标签不该让它白重建。
    #[test]
    fn revision_advances_on_queue_mutations_not_on_label_refresh() {
        let mut p = playlist(3);
        let after_push = p.revision();
        assert_eq!(p.push([track(0)]), 0, "同 id 是替换，不是新增");
        assert_eq!(p.revision(), after_push, "只刷新标签不该让行模型失效");

        assert!(p.jump_to(track(2).id));
        let after_jump = p.revision();
        assert_ne!(after_jump, after_push, "换当前曲要重建行模型");

        assert_eq!(p.next(), Advance::Stop);
        assert_eq!(p.revision(), after_jump, "游标已经在末尾，再前进是空操作");

        p.remove(track(1).id);
        assert_ne!(p.revision(), after_jump, "删歌改变了队列成分");
    }

    /// 队列定下来之后推进只在队列里走，走到头就停，不滑进整库的下一首。
    #[test]
    fn a_pinned_queue_walks_only_its_own_tracks() {
        let mut p = playlist(6);
        let picked: Vec<TrackId> = [3usize, 1, 4].iter().map(|n| track(*n).id).collect();
        p.set_queue(Some(picked), 7);

        assert_eq!(p.order().len(), 3, "整库 6 首，队列只有点进来的 3 首");
        assert_eq!(
            p.track_at_order(0).unwrap().id,
            track(3).id,
            "队列顺序 = 传进来的顺序，不是曲库顺序"
        );

        p.jump_to(track(3).id);
        assert!(matches!(p.next(), Advance::To(1)));
        assert_eq!(p.current_id(), Some(track(1).id));
        assert!(matches!(p.next(), Advance::To(2)));
        assert_eq!(p.current_id(), Some(track(4).id));
        assert_eq!(
            p.next(),
            Advance::Stop,
            "队列到头就该停，不该滑进整库的下一首"
        );
    }

    /// `None` 把队列还原成整库 —— 在曲库视图（无作用域、无搜索词）里点歌走的就是这条。
    #[test]
    fn clearing_the_queue_brings_the_whole_library_back() {
        let mut p = playlist(5);
        p.set_queue(Some(vec![track(2).id]), 7);
        assert_eq!(p.order().len(), 1);

        p.set_queue(None, 7);
        assert_eq!(p.queue(), None);
        assert_eq!(
            p.order().to_vec(),
            p.tracks().iter().map(|t| t.id).collect::<Vec<_>>()
        );
    }

    /// 界面直接把可见行喂进来，不该为「这批 id 一定干净」负责：不存在的丢掉，重复的只留第一次。
    #[test]
    fn a_queue_silently_drops_unknown_and_duplicate_ids() {
        let mut p = playlist(3);
        p.set_queue(
            Some(vec![track(1).id, track(99).id, track(1).id, track(0).id]),
            7,
        );
        assert_eq!(p.order().to_vec(), vec![track(1).id, track(0).id]);
    }

    /// 曲目被移出曲库时队列里那一份也要跟着消失，否则 `order` 会指向一首已经不在的歌。
    #[test]
    fn removing_a_track_drops_it_from_the_queue() {
        let mut p = playlist(3);
        p.set_queue(Some(vec![track(0).id, track(1).id, track(2).id]), 7);
        p.remove(track(1).id);

        assert_eq!(p.order().to_vec(), vec![track(0).id, track(2).id]);
        assert_eq!(p.queue().unwrap().len(), 2);
    }

    /// 随机只在队列内部打乱，不会把整库拉进来。
    #[test]
    fn shuffle_stays_inside_the_queue() {
        let mut p = playlist(20);
        let mut picked: Vec<TrackId> = (0..5).map(|n| track(n).id).collect();
        p.set_queue(Some(picked.clone()), 0x2545_f491_4f6c_dd1d);
        p.set_mode(PlayMode::Shuffle, 0x2545_f491_4f6c_dd1d);

        assert_eq!(p.order().len(), 5, "整库有 20 首，队列只该是那 5 首");
        let mut got = p.order().to_vec();
        got.sort_unstable();
        picked.sort_unstable();
        assert_eq!(got, picked, "洗牌只换顺序，不换集合");
    }

    /// 队列已定时导入新曲目不该挤进正在听的队列；还原成整库后才进来。
    #[test]
    fn importing_does_not_extend_a_pinned_queue() {
        let mut p = playlist(3);
        p.set_queue(Some(vec![track(0).id, track(1).id]), 7);

        assert_eq!(p.push([track(9)]), 1);
        assert_eq!(p.order().len(), 2, "新导入的曲目不该进已定的队列");
        assert_eq!(p.len(), 4);

        p.set_queue(None, 7);
        assert_eq!(p.order().len(), 4);
    }

    /// 队列已定时改排序键只动 `tracks`，不重排正在听的东西。
    #[test]
    fn sorting_does_not_reorder_a_pinned_queue() {
        let mut p = playlist(4);
        let picked = vec![track(2).id, track(0).id];
        p.set_queue(Some(picked.clone()), 7);

        p.sort_by(SortKey::Title);

        assert_eq!(
            p.order().to_vec(),
            picked,
            "队列是「按下播放那一刻看到的那批」，改排序不该动它"
        );
    }
}
