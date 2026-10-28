//! 曲库实体：播放列表、队列顺序、作用域与导入流程。
//! 曲目集合变了要走三件套：`persist` + `invalidate` + `recompute_visible`。

use std::collections::HashMap;
use std::rc::Rc;

use lyre_core::{
    AlbumGroup, ArtistGroup, Collections, PlayMode, Playlist, SortKey, Track, TrackId,
};

use crate::platform::*;

mod collections;
mod groups;
mod likes_follows;
mod mutation;
mod navigation;
mod playback_control;
mod query;
mod recent;
mod scan;
mod scope;

pub use collections::CollectionSummary;
pub use playback_control::{RepeatState, Step};
pub use query::searching;
pub use scan::ScanProgress;
pub use scope::{FAVORITES_NAME, Scope, ScopeKind};

#[derive(Clone, Debug, PartialEq)]
pub enum LibraryEvent {
    /// 该开始播这一首了
    Play(Track),
    /// 键盘光标移到了第 `usize` 行。滚动是副作用，不能放在 render 里做，只能用事件推到明确时机。
    FocusMoved(usize),
    /// 关注状态变了。详情页头图由 `RootView` 内联渲染、不 observe 实体，只能靠这个事件推重绘。
    FollowChanged,
    /// 作用域真的换了才发：`set_scope` 是唯一发出者，值没变不发；歌词遮罩只监听它。
    ScopeChanged,
    /// 一次导入扫完了。`scanned` 是扫到的首数，`added` 是新入库的；重导只刷新标签，added 可为 0。
    Imported { scanned: usize, added: usize },
    /// 启动时的曲库缓存已经读完并合并。续播要等它，才不会在空曲库里把 resume.json 清掉。
    Loaded,
    /// 一次操作没能完成，附带的文案直接给用户看（底部提示）。
    Failed(String),
}

pub struct LibraryEntity {
    playlist: Playlist,
    /// 键盘光标：`↑↓` 移动、`Enter` 才播。默认跟着播放走（`follow_playback`）。
    /// 不变量：要么 `None`，要么在可见行里，由 `recompute_visible` 维持。
    focus: Option<TrackId>,
    /// 底色该不该画：光标值一直在（`Enter` / `Delete` 要它），底色只在键盘真的动过之后才画。
    cursor_visible: bool,
    /// 实际装进播放器的那一首，不等于 `playlist.current_id()`（后者导入完就指向第一首）。
    now_playing: Option<TrackId>,
    query: String,
    /// 可见曲目在 `playlist.tracks()` 里的下标；`None` = 无过滤，不建恒等索引表。
    visible: Option<Vec<usize>>,
    /// 可见行号的 `TrackId` 反查，仅过滤生效时建立；键盘导航与选择清理靠它做到 O(1)。
    row_by_id: Option<HashMap<TrackId, usize>>,
    /// 预先小写化的检索字段，与 `playlist.tracks()` 同序；曲目集合变化时失效。
    search_fields: Option<Rc<Vec<crate::entities::filter::SearchFields>>>,
    /// 可见投影的代次。查询、作用域、删歌导致投影重算时加一，派生缓存按它失效。
    visible_generation: u64,
    /// 曲目 / 歌单 / 收藏 / 最近播放等数据内容的总代次；首页与搜索页的重型派生按它缓存。
    data_revision: u64,
    scope: Scope,
    scanning: Option<ScanProgress>,
    /// 分组缓存，`None` 表示需要重算。
    album_groups: Option<std::rc::Rc<Vec<AlbumGroup>>>,
    artist_groups: Option<std::rc::Rc<Vec<ArtistGroup>>>,
    /// 分组缓存的代次，[`Self::invalidate`] 每失效一次加一。
    groups_generation: u64,
    /// 洗牌种子：进随机时按固定乘加序列推进，决定洗牌顺序。
    seed: u64,
    /// 导入时是否递归子目录（来自设置）
    recursive: bool,
    /// 随机轴，另一轴是 [`Self::repeat`]，由 [`Self::effective_mode`] 折算成 `PlayMode`
    shuffle: bool,
    repeat: RepeatState,
    liked: crate::likes::Likes,
    /// 已关注的艺术家，按名字键控（不是 id）
    followed: crate::follows::Follows,
    collections: Collections,
    /// 最近播放，跨重启
    recents: crate::recents::Recents,
    /// 首屏还在等后台读 `library.json`。加载期间不落盘，避免把只有新导入项的曲库覆盖回去。
    loading: bool,
    /// 首屏加载任务，只写不读：留着它才能在实体销毁时取消。
    #[allow(dead_code)]
    load_task: Option<Task<()>>,
    /// 后台扫描任务，只写不读：留着它是为了 Drop 时取消扫描 —— `Task` 一 drop 就中止。
    _scan: Option<Task<()>>,
}

impl EventEmitter<LibraryEvent> for LibraryEntity {}

impl LibraryEntity {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // 窗口先出来：读盘、解析 JSON 与逐首核对文件都在后台线程做，完成后再合并。
        let mut this = Self {
            playlist: Playlist::new(),
            focus: None,
            cursor_visible: false,
            now_playing: None,
            query: String::new(),
            visible: None,
            row_by_id: None,
            search_fields: None,
            visible_generation: 0,
            data_revision: 0,
            scope: Scope::All,
            scanning: None,
            album_groups: None,
            artist_groups: None,
            groups_generation: 0,
            seed: 0x2545_f491_4f6c_dd1d,
            recursive: true,
            shuffle: false,
            repeat: RepeatState::Off,
            liked: crate::likes::Likes::load(),
            followed: crate::follows::Follows::load(),
            collections: crate::collections_store::load(),
            recents: crate::recents::Recents::load(),
            loading: true,
            load_task: None,
            _scan: None,
        };

        let executor = cx.background_executor().clone();
        let load = cx.spawn(async move |this, cx| {
            let tracks = executor
                .spawn(async move { crate::library_store::load() })
                .await;
            this.update(cx, |this, cx| this.finish_initial_load(tracks, cx))
                .ok();
        });
        this.load_task = Some(load);
        this
    }

    /// 首屏曲库是否仍在后台读取。
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// 合并后台读回的曲库。加载期间用户已经导入的曲目排在后面，不丢任何一边。
    fn finish_initial_load(&mut self, loaded: Vec<lyre_core::Track>, cx: &mut Context<Self>) {
        if !self.loading {
            return;
        }
        let pending: Vec<lyre_core::Track> = self.playlist.tracks().to_vec();
        let had_pending = !pending.is_empty();
        self.playlist = merge_tracks(loaded, pending);
        self.loading = false;

        self.invalidate();
        self.recompute_visible(cx);
        if had_pending {
            // 加载期间用户导入的曲目要补写；没有新导入就不必每次启动都重写整库。
            self.persist();
        }
        cx.emit(LibraryEvent::Loaded);
        cx.notify();
    }

    /// 把曲库按导入顺序写盘。曲目集合变化的每个出口都要调一次；排序与播放模式不落盘。
    /// 首屏加载完成前不写：这时内存里只有加载期间新导入的几首，写下去会把整库覆盖掉。
    fn persist(&self) {
        if self.loading {
            return;
        }
        crate::library_store::save(self.playlist.in_import_order().cloned().collect());
    }

    pub fn playlist(&self) -> &Playlist {
        &self.playlist
    }

    /// 全部曲目（未经搜索过滤）
    pub fn tracks(&self) -> &[Track] {
        self.playlist.tracks()
    }

    /// 曲库总数（未经过滤）
    pub fn len(&self) -> usize {
        self.playlist.len()
    }

    pub fn is_empty(&self) -> bool {
        self.playlist.is_empty()
    }

    pub fn mode(&self) -> PlayMode {
        self.playlist.mode()
    }

    pub fn sort_key(&self) -> SortKey {
        self.playlist.sort_key()
    }

    /// 数据内容代次：曲目、歌单、收藏、最近播放任一变化都会前进。派生缓存用它做键。
    pub fn data_revision(&self) -> u64 {
        self.data_revision
    }

    pub(super) fn touch_data(&mut self) {
        self.data_revision = self.data_revision.wrapping_add(1);
    }
}

/// 后台读回的曲库在前，加载期间新导入的曲目在后；同 id 用新导入的标签覆盖。
fn merge_tracks(loaded: Vec<Track>, pending: Vec<Track>) -> Playlist {
    let mut playlist = Playlist::new();
    playlist.push(loaded);
    playlist.push(pending);
    playlist
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`crate::platform::*` 带的 gpui `test` 宏会遮蔽内置 `#[test]`。
    use std::path::{Path, PathBuf};

    use lyre_core::{Track, TrackId};

    use super::merge_tracks;

    fn track(name: &str, title: &str) -> Track {
        let path = PathBuf::from(format!("/music/{name}.flac"));
        let mut track = Track::fallback(&path);
        track.title = title.to_owned();
        track
    }

    #[test]
    fn loaded_tracks_stay_before_anything_imported_while_loading() {
        let merged = merge_tracks(
            vec![track("a", "旧 A"), track("b", "旧 B")],
            vec![track("c", "新 C")],
        );
        let titles: Vec<&str> = merged.tracks().iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["旧 A", "旧 B", "新 C"]);
    }

    #[test]
    fn a_track_imported_while_loading_refreshes_the_loaded_label() {
        let merged = merge_tracks(vec![track("a", "旧标题")], vec![track("a", "重导后的标题")]);
        assert_eq!(merged.len(), 1, "同路径不该变成两首");
        assert_eq!(merged.tracks()[0].title, "重导后的标题");
        assert_eq!(
            merged.tracks()[0].id,
            TrackId::from_path(Path::new("/music/a.flac"))
        );
    }
}
