//! 主区在看曲库的哪一部分（作用域）及其显示名。
//! `set_scope` 是 [`super::LibraryEvent::ScopeChanged`] 的唯一发出者，值没变就不发。

use lyre_core::CollectionId;

use crate::platform::*;

use super::{LibraryEntity, LibraryEvent};

/// 「我喜欢的音乐」的显示名，侧栏 / 面包屑 / 后退提示共用这一处。
pub const FAVORITES_NAME: &str = "我喜欢的音乐";

/// 主区正在看曲库的哪一部分。与搜索正交：作用域定义容器，搜索在容器里再筛。
/// 专辑 / 艺术家按名字圈定，歌单按 id 圈定 —— 歌单名可重复也可改，按名字定位会让导航历史指错。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    /// 只看某一张专辑
    Album(String),
    /// 只看某一位艺术家
    Artist(String),
    /// 只看某个自建歌单
    Collection(CollectionId),
    /// 「我喜欢的音乐」：成员由收藏决定，不是一个真的歌单
    Favorites,
}

impl Scope {
    pub fn is_scoped(&self) -> bool {
        !matches!(self, Self::All)
    }

    /// 歌单与收藏都归 [`ScopeKind::All`]：曲目不来自单一容器，专辑列不能省。
    pub fn kind(&self) -> ScopeKind {
        match self {
            Self::All | Self::Collection(_) | Self::Favorites => ScopeKind::All,
            Self::Album(_) => ScopeKind::Album,
            Self::Artist(_) => ScopeKind::Artist,
        }
    }
}

/// [`Scope`] 的种类，剥掉名字：`Copy`，行渲染每帧取用而不 clone `String`。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScopeKind {
    #[default]
    All,
    Album,
    Artist,
}

impl LibraryEntity {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn enter_album(&mut self, title: impl Into<String>, cx: &mut Context<Self>) {
        self.set_scope(Scope::Album(title.into()), cx);
    }

    pub fn enter_artist(&mut self, name: impl Into<String>, cx: &mut Context<Self>) {
        self.set_scope(Scope::Artist(name.into()), cx);
    }

    /// 进入一个歌单。id 查不到时什么都不做 —— 点到的卡片可能已经被删掉了。
    pub fn enter_collection(&mut self, id: CollectionId, cx: &mut Context<Self>) {
        if self.collections.get(id).is_some() {
            self.set_scope(Scope::Collection(id), cx);
        }
    }

    pub fn enter_favorites(&mut self, cx: &mut Context<Self>) {
        self.set_scope(Scope::Favorites, cx);
    }

    /// 当前作用域的面包屑文字。
    pub fn scope_label(&self) -> Option<String> {
        self.label_for(&self.scope)
    }

    /// 把任意一个作用域翻成显示文字；后退提示要问的是栈顶那个落点，不是当前。
    /// 歌单名不在 `Scope` 里（只存 id），所以每次查一遍，改名立刻生效。
    pub fn label_for(&self, scope: &Scope) -> Option<String> {
        match scope {
            Scope::All => None,
            Scope::Album(title) | Scope::Artist(title) => Some(title.clone()),
            Scope::Collection(id) => self.collections.get(*id).map(|list| list.name.clone()),
            Scope::Favorites => Some(FAVORITES_NAME.to_owned()),
        }
    }

    /// 回到整个曲库。搜索词不清：它是另一个维度的条件。
    pub fn exit_scope(&mut self, cx: &mut Context<Self>) {
        self.set_scope(Scope::All, cx);
    }

    /// 把作用域恢复到导航历史里的落点。歌单可能已被删掉，id 要重新验一次，验不过就退成 `All`。
    pub fn restore_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        let scope = match scope {
            Scope::Collection(id) if self.collections.get(id).is_none() => Scope::All,
            other => other,
        };
        self.set_scope(scope, cx);
    }

    pub(super) fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        self.recompute_visible(cx);
        cx.emit(LibraryEvent::ScopeChanged);
        cx.notify();
    }
}
