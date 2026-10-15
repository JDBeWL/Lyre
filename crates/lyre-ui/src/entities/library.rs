//! 曲库实体：播放列表、队列顺序、作用域与导入流程。
//! 曲目集合变了要走三件套：`persist` + `invalidate` + `recompute_visible`。

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
    /// 后台扫描任务，只写不读：留着它是为了 Drop 时取消扫描 —— `Task` 一 drop 就中止。
    _scan: Option<Task<()>>,
}

impl EventEmitter<LibraryEvent> for LibraryEntity {}

impl LibraryEntity {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let _ = cx;
        // 上次导入的曲库直接回来；文件已被移走 / 删掉的条目在 load 里剔掉。
        let mut playlist = Playlist::new();
        playlist.push(crate::library_store::load());

        Self {
            playlist,
            focus: None,
            cursor_visible: false,
            now_playing: None,
            query: String::new(),
            visible: None,
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
            _scan: None,
        }
    }

    /// 把曲库按导入顺序写盘。曲目集合变化的每个出口都要调一次；排序与播放模式不落盘。
    fn persist(&self) {
        crate::library_store::save(&self.playlist.in_import_order().collect::<Vec<_>>());
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
}
