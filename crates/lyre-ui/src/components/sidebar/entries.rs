//! 侧栏列表的条目模型与一行的渲染。
//! 条目的身份 [`SideEntry::key`] 与标题分开：专辑、艺术家、歌单都允许重名。

use std::path::PathBuf;

use lyre_core::{AlbumGroup, ArtistGroup, TrackId};

use crate::common::{thousands, tooltip};
use crate::cover::CoverCache;
// `Scope` 要显式导入：`platform::*` 带着 `gpui::Scope<'_>`，靠 glob 引进来会撞名。
use crate::entities::{CollectionSummary, LibraryEntity, Scope};
use crate::icon::{Glyph, icon, icon_inherit};
use crate::platform::*;
use crate::shell::{NavSection, ShellEntity};
use crate::theme;

use super::Sidebar;
use super::menu::collection_menu;
use super::thumb::row_thumb;

/// 一行是「标题 + 副标题」两行，`SIDEBAR_ITEM_H`（40）塞不下；48 = 上下各 6px + 32px 缩略图。
const LIB_ROW_H: f32 = 48.0;

/// 侧栏列表该列哪一种粒度。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Listing {
    Albums,
    Artists,
    Collections,
}

/// 主区在 `nav` 时侧栏该列什么：`Albums` 列专辑，其余区段（含 `Tracks`）都列艺术家。
pub(super) fn listing_for(nav: NavSection) -> Listing {
    match nav {
        NavSection::Albums => Listing::Albums,
        NavSection::Home | NavSection::Tracks | NavSection::Artists | NavSection::Search => {
            Listing::Artists
        }
        // 设置页不参与目录粒度
        NavSection::Settings => Listing::Artists,
    }
}

/// 侧栏列表里的一行 —— 专辑、艺术家、歌单或收藏。
#[derive(Clone)]
pub(super) struct SideEntry {
    /// 行的稳定标识，用作元素 id 与 group 名。不能用标题：专辑、艺术家、歌单都可能重名，
    /// 撞了 id 的两行会共享悬停态。
    key: String,
    /// 用来「播这一行」的代表曲目。空歌单没有，此时不画播放键。
    pub(super) id: Option<lyre_core::TrackId>,
    /// 歌单封面那一格的代表曲目：第一个带内嵌封面的成员，而不是第一首。
    pub(super) cover_id: Option<lyre_core::TrackId>,
    /// 用户为这个歌单指定的封面文件，优先级高于内嵌封面。
    pub(super) cover_file: Option<PathBuf>,
    pub(super) title: String,
    /// 副标题：专辑给艺术家，艺术家给「N 张专辑」。
    subtitle: String,
    /// 进入时用哪个作用域（专辑名或艺术家名）。主区显示什么不由它决定：
    /// [`crate::shell::enter_scope`] 一律切到曲库页，`AlbumGrid` 不读 `Scope`。
    pub(super) scope: SideScope,
}

/// 点一行之后收窄到哪种作用域。歌单带 id 而不是名字：名字可以重复也可以被改。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SideScope {
    Album,
    Artist,
    Collection(lyre_core::CollectionId),
    Favorites,
    /// 「+ 新建歌单」那一行，点了不收窄作用域，而是建一个再进去
    NewCollection,
}

impl SideScope {
    /// 这一行进的是哪个作用域。专辑 / 艺术家的名字从行标题来，歌单从 id 来。
    pub(super) fn scope_of(self, title: &str) -> Option<Scope> {
        match self {
            Self::Album => Some(Scope::Album(title.to_owned())),
            Self::Artist => Some(Scope::Artist(title.to_owned())),
            Self::Collection(id) => Some(Scope::Collection(id)),
            Self::Favorites => Some(Scope::Favorites),
            Self::NewCollection => None,
        }
    }
}

impl SideEntry {
    fn from_album(group: &AlbumGroup) -> Self {
        let artist = group.display_artist();
        let subtitle = if artist.is_empty() {
            format!("{} 首", thousands(group.track_count()))
        } else {
            format!("{} · {} 首", artist, thousands(group.track_count()))
        };
        Self {
            key: format!("album:{}", group.title),
            id: Some(group.representative),
            cover_id: None,
            cover_file: None,
            title: group.title.clone(),
            subtitle,
            scope: SideScope::Album,
        }
    }

    fn from_artist(group: &ArtistGroup) -> Self {
        Self {
            key: format!("artist:{}", group.name),
            id: Some(group.representative),
            cover_id: None,
            cover_file: None,
            title: group.name.clone(),
            subtitle: format!("{} 张专辑", thousands(group.album_count())),
            scope: SideScope::Artist,
        }
    }
}

impl SideEntry {
    /// 「我喜欢的音乐」那一行。排在歌单列表最前，没有删除入口 —— 成员由爱心决定。
    fn favorites(count: usize) -> Self {
        Self {
            key: "favorites".to_owned(),
            id: None,
            cover_id: None,
            cover_file: None,
            title: crate::entities::FAVORITES_NAME.to_owned(),
            subtitle: format!("歌单 · {count} 首"),
            scope: SideScope::Favorites,
        }
    }

    fn collection(id: lyre_core::CollectionId, name: &str, summary: CollectionSummary) -> Self {
        Self {
            key: format!("collection:{}", id.0),
            // 点这一行不收窄到某一首，所以没有「播这一行」的落点；封面另有 `cover_id`。
            id: None,
            cover_id: summary.cover_id,
            cover_file: summary.cover_file,
            title: name.to_owned(),
            subtitle: format!("歌单 · {} 首", summary.count),
            scope: SideScope::Collection(id),
        }
    }

    fn new_collection() -> Self {
        Self {
            key: "new-collection".to_owned(),
            id: None,
            cover_id: None,
            cover_file: None,
            title: "新建歌单".to_owned(),
            subtitle: String::new(),
            scope: SideScope::NewCollection,
        }
    }
}

impl Sidebar {
    pub(super) fn entries(&self, nav: NavSection, cx: &mut App) -> Vec<SideEntry> {
        match self.effective_listing(nav) {
            Listing::Artists => {
                let groups = App::update_entity(cx, &self.library, |library, _| library.artists());
                groups.iter().map(SideEntry::from_artist).collect()
            }
            Listing::Albums => {
                let groups = App::update_entity(cx, &self.library, |library, _| library.albums());
                groups.iter().map(SideEntry::from_album).collect()
            }
            // 数字与封面按当前曲库里真的还有哪几首算，不是歌单自己的成员表。
            Listing::Collections => {
                let (liked, lists) = App::update_entity(cx, &self.library, |library, _| {
                    let liked = library.liked_count();
                    let lists: Vec<_> = library
                        .collections()
                        .iter()
                        .map(|list| {
                            (
                                list.id,
                                list.name.clone(),
                                library.collection_summary(list.id),
                            )
                        })
                        .collect();
                    (liked, lists)
                });
                let mut rows = vec![SideEntry::favorites(liked)];
                rows.extend(
                    lists
                        .into_iter()
                        .map(|(id, name, summary)| SideEntry::collection(id, &name, summary)),
                );
                rows.push(SideEntry::new_collection());
                rows
            }
        }
    }
}

pub(super) fn lib_row(
    entry: SideEntry,
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
    covers: &Entity<CoverCache>,
    cx: &mut App,
) -> AnyElement {
    // 判据只有「作用域标签等于这一行标题，且种类对得上」—— 不掺 `nav_section`：
    // 同名的专辑和艺术家不是同一个东西。
    let selected = {
        let scope = App::update_entity(cx, &library, |library, _| library.scope().clone());
        match (&entry.scope, &scope) {
            (SideScope::Album, Scope::Album(title)) => title == &entry.title,
            (SideScope::Artist, Scope::Artist(name)) => name == &entry.title,
            (SideScope::Collection(id), Scope::Collection(known)) => id == known,
            (SideScope::Favorites, Scope::Favorites) => true,
            _ => false,
        }
    };

    let thumb = row_thumb(&entry, covers, &library, cx);
    let group = SharedString::from(format!("side-row-group-{}", entry.key));
    let play_title = entry.title.clone();
    let play_library = library.clone();
    let menu_library = library.clone();
    let menu_title = entry.title.clone();
    let row_scope = entry.scope;

    let mut row = div()
        .id(SharedString::from(format!("side-row-{}", entry.key)))
        .group(group.clone())
        .flex_none()
        .h(px(LIB_ROW_H))
        // `w_full` + `min_w_0` 缺一不可：`uniform_list` 的行默认按内容定宽，
        // 长标题会把行顶宽、不截断，底色也铺不满整行。
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(theme::GAP_SM))
        .rounded(px(theme::RADIUS_MD))
        .cursor_pointer()
        .px(px(theme::GAP_XS))
        .bg(if selected {
            theme::elevated()
        } else {
            theme::transparent()
        })
        .hover(|style| style.bg(theme::elevated()))
        .on_click({
            let entry = entry.clone();
            move |_, _, cx| {
                // 「新建」那一行先建出来再进去，名字带序号 —— 否则连着点会出一排同名，侧栏分不清
                let scope = entry.scope.scope_of(&entry.title).or_else(|| {
                    library.update(cx, |library, cx| {
                        let name = format!("新歌单 {}", library.collections().len() + 1);
                        library.create_collection(&name, cx).map(Scope::Collection)
                    })
                });
                let Some(scope) = scope else { return };

                // 压历史必须在收窄之前，否则返回没有落点。
                let here = {
                    let section = shell.read(cx).nav_section();
                    let current =
                        App::update_entity(cx, &library, |library, _| library.scope().clone());
                    (section, current)
                };
                shell.update(cx, |shell, cx| shell.push_history(here.0, here.1, cx));
                crate::shell::enter_scope(scope, &shell, &library, cx);
            }
        })
        .child(thumb)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        // tooltip 要求元素有 id；与 `side-row-*` 同源派生，重名专辑也不撞 id。
                        .id(SharedString::from(format!("side-title-{}", entry.key)))
                        .text_size(px(theme::FONT_TRACK))
                        .font_weight(if selected {
                            theme::WEIGHT_BUTTON
                        } else {
                            theme::WEIGHT_TRACK
                        })
                        .text_color(if selected {
                            theme::text()
                        } else {
                            theme::text_muted()
                        })
                        .truncate()
                        .tooltip(crate::common::tooltip::builder(entry.title.clone()))
                        .child(entry.title),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(theme::text_faint())
                        .truncate()
                        .child(entry.subtitle),
                ),
        );

    if selected {
        row = row.child(
            div()
                .flex_none()
                .pr(px(theme::GAP_XS))
                .text_color(theme::accent_text())
                .child(icon_inherit(Glyph::VolumeHigh, 14.)),
        );
    } else if let Some(id) = entry.id {
        row = row.child(row_play_button(group, id, &play_title, play_library));
    }

    // 只有自建歌单有右键菜单：收藏不能改名也不能删（成员由爱心决定），
    // 「新建歌单」那一行更是没有可操作的对象。
    match row_scope {
        SideScope::Collection(id) => row
            .context_menu(collection_menu(menu_library, id, menu_title))
            .into_any_element(),
        _ => row.into_any_element(),
    }
}

/// 行尾的悬浮播放键：常驻占位，靠 `group_hover` 改透明度显隐。
/// 它嵌在「点开分组」的整行里，必须阻断冒泡，否则一次点击同时播放并跳详情。
fn row_play_button(
    group: SharedString,
    id: TrackId,
    title: &str,
    library: Entity<LibraryEntity>,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("side-play-{}", id.0)))
        .flex_none()
        .mr(px(theme::GAP_XS))
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme::control())
        .opacity(0.0)
        .cursor_pointer()
        .group_hover(group, |style| style.opacity(1.0))
        .hover(|style| style.bg(theme::control_hover()))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            library.update(cx, |library, cx| library.activate(id, cx));
        })
        .tooltip(tooltip::builder(format!("播放《{title}》")))
        .child(icon(Glyph::Play, 14., theme::text_on_control()))
        .into_any_element()
}
