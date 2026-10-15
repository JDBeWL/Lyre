//! 专辑 / 艺术家的惰性分组缓存：分组是整库 O(n log n) 扫描，只在读时重算。
//! 曲目集合一变就由 [`LibraryEntity::invalidate`] 置空，每个改动出口都必须调它。

use std::rc::Rc;

use lyre_core::{AlbumGroup, ArtistGroup, group_by_album, group_by_artist};

use super::LibraryEntity;

impl LibraryEntity {
    /// 专辑分组，带缓存：侧栏计数与网格每帧都要它，而 `group_by_album` 要遍历整库。
    /// 返回 `Rc` 是因为调用方要把它带进 `'static` 的点击闭包，借不出来。
    pub fn albums(&mut self) -> Rc<Vec<AlbumGroup>> {
        if let Some(cached) = &self.album_groups {
            Rc::clone(cached)
        } else {
            let computed = Rc::new(group_by_album(self.playlist.tracks()));
            self.album_groups = Some(Rc::clone(&computed));
            computed
        }
    }

    pub fn artists(&mut self) -> Rc<Vec<ArtistGroup>> {
        if let Some(cached) = &self.artist_groups {
            Rc::clone(cached)
        } else {
            let computed = Rc::new(group_by_artist(self.playlist.tracks()));
            self.artist_groups = Some(Rc::clone(&computed));
            computed
        }
    }

    pub fn album_count(&self) -> usize {
        // 没算过就回 0，不为一个计数遍历整库
        self.album_groups.as_ref().map_or(0, |g| g.len())
    }

    pub fn artist_count(&self) -> usize {
        self.artist_groups.as_ref().map_or(0, |g| g.len())
    }

    /// 让分组缓存失效。曲目集合变化的增删路径都必须走这里，别处不该置空缓存。
    pub(super) fn invalidate(&mut self) {
        self.album_groups = None;
        self.artist_groups = None;
        self.groups_generation = self.groups_generation.wrapping_add(1);
    }

    /// 分组缓存的代次，派生视图（网格行、首页区块）用它做缓存键。
    pub fn groups_generation(&self) -> u64 {
        self.groups_generation
    }
}
