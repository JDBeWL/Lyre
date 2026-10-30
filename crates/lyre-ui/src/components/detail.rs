//! 专辑 / 艺术家 / 歌单详情页的头图。

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use lyre_core::{AlbumGroup, ArtistGroup, CollectionId, TrackId, format_duration};

use crate::common::thousands;
use crate::cover::CoverCache;
use crate::entities::{FAVORITES_NAME, LibraryEntity, Scope};
use crate::icon::{Glyph, icon, icon_inherit};
use crate::platform::*;
use crate::shell::{PanelTab, ShellEntity};
use crate::theme;

/// 头图封面的边长：再大会在小窗口里把曲目列表整个挤出视野。
const COVER: f32 = 180.0;

const PLAY_BTN: f32 = 52.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeroKind {
    Album,
    Artist,
    /// 自建歌单与「我喜欢的音乐」。歌单没有内嵌封面，封面一律派生。
    Collection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeroInfo {
    pub kind: HeroKind,
    /// 大标题：专辑名 / 艺术家名。
    pub title: String,
    /// 分类标签：「专辑」/「艺术家」/「歌单」。
    pub tag: &'static str,
    /// 副标题：专辑给艺术家名，艺术家给「N 张专辑」。
    pub subtitle: String,
    /// 说明行：「12 首 · 42:15」。时长未知时只有曲目数。
    pub meta: String,
    /// 取封面的代表曲目。空歌单没有曲目可代表，`None` 时画派生色块。
    pub cover_id: Option<TrackId>,
    /// 用户给歌单指定的封面（数据目录里的拷贝）。有它就优先于内嵌封面。
    pub cover_file: Option<PathBuf>,
    /// 只有歌单头图需要：「更换封面」得知道改哪一个。
    pub collection_id: Option<CollectionId>,
}

impl HeroInfo {
    pub fn from_album(group: &AlbumGroup) -> Self {
        let artist = group.display_artist();
        let tracks = group.track_count();
        let meta = match group.total_duration() {
            Some(total) => format!("{} 首 · {}", thousands(tracks), format_duration(total)),
            None => format!("{} 首", thousands(tracks)),
        };
        Self {
            kind: HeroKind::Album,
            title: group.title.clone(),
            tag: "专辑",
            subtitle: artist,
            meta,
            cover_id: Some(group.representative),
            cover_file: None,
            collection_id: None,
        }
    }

    pub fn from_artist(group: &ArtistGroup) -> Self {
        let (tracks, albums) = (group.track_count(), group.album_count());
        let meta = match group.total_duration() {
            Some(total) => format!("{} 首 · {}", thousands(tracks), format_duration(total)),
            None => format!("{} 首", thousands(tracks)),
        };
        Self {
            kind: HeroKind::Artist,
            title: group.name.clone(),
            tag: "艺术家",
            subtitle: format!("{} 张专辑", thousands(albums)),
            meta,
            cover_id: Some(group.representative),
            cover_file: None,
            collection_id: None,
        }
    }

    /// 「关注」的对象名。艺术家页取标题；专辑页取唯一艺术家名。
    /// 群星 / 无艺术家（`subtitle` 为空或 [`lyre_core::VARIOUS_ARTISTS`]）返回 `None`。
    pub fn follow_target(&self) -> Option<&str> {
        match self.kind {
            HeroKind::Artist => Some(self.title.as_str()),
            HeroKind::Collection => None,
            HeroKind::Album => {
                let artist = self.subtitle.trim();
                if artist.is_empty() || artist == lyre_core::VARIOUS_ARTISTS {
                    return None;
                }
                // 多人字段拆开后每一位才是艺术家页的名字，整串不是任何一个人的名字，匹配不上。
                match lyre_core::artist_keys_str(Some(artist)).as_slice() {
                    [one] => Some(one),
                    _ => None,
                }
            }
        }
    }
}

/// 按当前作用域取头图数据；找不到对应分组返回 `None`，调用方退回不画头图。
pub fn hero_for(library: &mut LibraryEntity, scope: &Scope) -> Option<HeroInfo> {
    match scope {
        Scope::All => None,
        Scope::Album(title) => library
            .albums()
            .iter()
            .find(|group| &group.title == title)
            .map(HeroInfo::from_album),
        Scope::Artist(name) => library
            .artists()
            .iter()
            .find(|group| &group.name == name)
            .map(HeroInfo::from_artist),
        Scope::Favorites => Some(list_hero(
            library,
            None,
            FAVORITES_NAME.to_owned(),
            library.liked_tracks(),
            "收藏的曲目",
        )),
        Scope::Collection(id) => {
            let (title, members) = library.collection_members(*id)?;
            Some(list_hero(library, Some(*id), title, members, ""))
        }
    }
}

/// 拼一个歌单类头图。计数只算当前曲库里解析得出的成员 —— 幽灵行进计数的话「12 首」点进去只有 9 行。
/// 封面取第一个有内嵌封面的成员，一个都没有时用派生色块。
fn list_hero(
    library: &LibraryEntity,
    id: Option<CollectionId>,
    title: String,
    members: Vec<TrackId>,
    subtitle: &str,
) -> HeroInfo {
    let cover_file = id.and_then(|id| library.collection_cover(id));
    let mut count = 0usize;
    let mut total = Duration::ZERO;
    let mut complete = true;
    let mut cover: Option<TrackId> = None;
    for id in members {
        let Some(track) = library.playlist().by_id(id) else {
            continue;
        };
        count += 1;
        match track.duration {
            Some(duration) => total += duration,
            None => complete = false,
        }
        if cover.is_none() && track.has_cover {
            cover = Some(id);
        }
    }

    let meta = if count == 0 {
        "还没有曲目".to_owned()
    } else if complete {
        format!("{} 首 · {}", thousands(count), format_duration(total))
    } else {
        format!("{} 首", thousands(count))
    };

    HeroInfo {
        kind: HeroKind::Collection,
        title,
        tag: "歌单",
        subtitle: subtitle.to_owned(),
        meta,
        cover_id: cover,
        cover_file,
        collection_id: id,
    }
}

/// 详情页「更多操作」里的多选开关。列表实体拥有状态，头图只负责把菜单项画出来。
pub struct MultiSelectAction {
    pub selecting: bool,
    pub on_toggle: Rc<dyn Fn(&mut App) + 'static>,
}

/// 渲染详情页头图：`kind` 只影响封面形状（专辑方形 / 艺术家圆形）。
pub fn hero(
    info: &HeroInfo,
    library: Entity<LibraryEntity>,
    covers: &Entity<CoverCache>,
    shell: Entity<ShellEntity>,
    multi_select: Option<MultiSelectAction>,
    cx: &mut App,
) -> AnyElement {
    let cover = cover_slot(covers, &library, info, cx);
    let actions = action_row(info, library, shell, multi_select, cx);

    let tag = div()
        .text_size(px(theme::FONT_TITLE))
        .font_weight(theme::WEIGHT_TITLE)
        .text_color(theme::text())
        .child(info.tag);

    div()
        .flex_none()
        .flex()
        .flex_col()
        .px(px(theme::GAP_LG))
        // 顶行顶到与按钮同一高度；让位的量与按钮位置同源于 `root::HEADER_BACK_INSET`。
        .pt(px(crate::root::BACK_INSET_TOP))
        .pb(px(theme::GAP_LG))
        .child(
            div()
                .h(px(crate::root::BACK_BTN))
                .flex()
                .items_center()
                .pl(px(
                    crate::root::HEADER_BACK_INSET - theme::GAP_LG + theme::GAP_SM
                ))
                .child(tag),
        )
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(theme::GAP_XL))
                .mt(px(theme::GAP_LG))
                .child(cover)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(theme::GAP_SM))
                        // 父级 `items_end` 只让文字块整体贴底，这里要 `justify_end` 把自己推到底。
                        .justify_end()
                        .pb(px(6.0))
                        .child(
                            div()
                                .text_size(px(theme::FONT_HERO))
                                .font_weight(theme::WEIGHT_TITLE)
                                .text_color(theme::text())
                                // 必须截断：gpui 默认换行，`hero` 是 `flex_none`、列表是 `flex_1`，
                                // 长专辑名多折一行就吃掉列表一行，极端情况能吃掉整屏。
                                .truncate()
                                .child(info.title.clone()),
                        )
                        .when(!info.subtitle.trim().is_empty(), |col| {
                            col.child(
                                div()
                                    .text_size(px(theme::FONT_SUBTITLE))
                                    .text_color(theme::text_muted())
                                    .child(info.subtitle.clone()),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(theme::FONT_CAPTION))
                                .text_color(theme::text_faint())
                                .child(info.meta.clone()),
                        )
                        .child(div().mt(px(theme::GAP_XS)).child(actions)),
                ),
        )
        .into_any_element()
}

fn action_row(
    info: &HeroInfo,
    library: Entity<LibraryEntity>,
    shell: Entity<ShellEntity>,
    multi_select: Option<MultiSelectAction>,
    cx: &mut App,
) -> AnyElement {
    // gpui 0.3.8 没有 `flex_wrap`，一行放不下只能让 chips 收缩，不能换行。
    // 收缩顺序由 `min_w_0` 决定：两颗 chip 的文字截断，「播放」与「更多」保住完整尺寸。
    let mut row = div()
        .flex()
        .items_center()
        .min_w_0()
        .gap(px(theme::GAP_MD))
        .child(play_button(info, library.clone()));

    if let Some(target) = info.follow_target().map(str::to_owned) {
        let following = library.read(cx).is_following(&target);
        row = row.child(follow_button(target, following, library.clone()));
    }

    row.child(shuffle_button(info, library.clone()))
        .child(more_button(
            shell,
            library,
            info.collection_id,
            multi_select,
        ))
        .into_any_element()
}

/// 关注 / 已关注。名字是键，切换后由 `library` 的事件推 [`crate::root::RootView`] 重绘。
fn follow_button(name: String, following: bool, library: Entity<LibraryEntity>) -> AnyElement {
    let tip = if following {
        format!("取消关注 {name}")
    } else {
        format!("关注 {name}")
    };

    crate::common::button::toggle_chip(
        "hero-follow",
        if following {
            Glyph::HeartFilled
        } else {
            Glyph::Heart
        },
        if following { "已关注" } else { "关注" },
        following,
    )
    .tooltip(crate::common::tooltip::builder(tip))
    .on_click(move |_, _, cx| {
        library.update(cx, |library, cx| {
            library.toggle_follow(name.clone(), cx);
        });
    })
    .into_any_element()
}

/// 随机播放：先打开随机轴，再从可见区第一行开始播。
fn shuffle_button(info: &HeroInfo, library: Entity<LibraryEntity>) -> AnyElement {
    let label = match info.kind {
        HeroKind::Album => format!("随机播放《{}》", info.title),
        HeroKind::Artist => format!("随机播放 {}", info.title),
        HeroKind::Collection => format!("随机播放歌单「{}」", info.title),
    };

    crate::common::button::toggle_chip("hero-shuffle", Glyph::Shuffle, "随机播放", false)
        .tooltip(crate::common::tooltip::builder(label))
        .on_click(move |_, _, cx| {
            library.update(cx, |library, cx| {
                // 用 set_shuffle 而不是 toggle：已经是随机时，再点一次不该把它关掉。
                library.set_shuffle(true, cx);
                // 挑第一首能播的，而不是死盯第一行：专辑里第一首可能正是引擎解不了的格式。
                library.play_first_playable(cx);
            });
        })
        .into_any_element()
}

/// 更多：只放头图这一排装不下的动作，不重复「播放 / 随机 / 关注」。
/// 「分享」本仓库没有对应能力，做成禁用态占位。
fn more_button(
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
    collection: Option<CollectionId>,
    multi_select: Option<MultiSelectAction>,
) -> AnyElement {
    let trigger = div()
        .id("hero-more")
        .flex_none()
        .size(px(34.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_PILL))
        .border_1()
        .border_color(theme::border())
        .text_color(theme::text())
        .cursor_pointer()
        .hover(|style| style.bg(theme::elevated()))
        .tooltip(crate::common::tooltip::builder("更多操作"))
        .child(icon_inherit(Glyph::More, 16.));

    crate::common::menu::dropdown(trigger, Anchor::TopLeft, move |menu, _window, _cx| {
        let open_queue = shell.clone();
        let open_now = shell.clone();
        let mut menu = menu
            .item(
                PopupMenuItem::new("打开播放队列").on_click(move |_, _, cx| {
                    open_queue.update(cx, |shell, cx| shell.toggle_panel(PanelTab::Queue, cx));
                }),
            )
            .item(
                PopupMenuItem::new("打开正在播放").on_click(move |_, _, cx| {
                    open_now.update(cx, |shell, cx| shell.toggle_panel(PanelTab::NowPlaying, cx));
                }),
            );

        if let Some(action) = multi_select.as_ref() {
            let label = if action.selecting {
                "退出多选"
            } else {
                "多选"
            };
            let toggle = Rc::clone(&action.on_toggle);
            menu = menu
                .separator()
                .item(PopupMenuItem::new(label).on_click(move |_, _, cx| toggle(cx)));
        }

        menu.when_some(collection, |menu, id| {
            let library = library.clone();
            menu.separator()
                .item(PopupMenuItem::new("更换封面").on_click(move |_, _, cx| {
                    library.update(cx, |library, cx| library.pick_collection_cover(id, cx));
                }))
        })
        .item(PopupMenuItem::new("分享（暂未支持）").disabled(true))
    })
}

/// 把光标落到可见区第一行再播它，之后按队列顺序往下走。
/// 播放队列恒等于曲库顺序（见 `LibraryEntity::set_query`：搜索只改可见性，不改队列）。
fn play_button(info: &HeroInfo, library: Entity<LibraryEntity>) -> AnyElement {
    let label = match info.kind {
        HeroKind::Album => format!("从《{}》的第一首开始播放", info.title),
        HeroKind::Artist => format!("从 {} 的第一首开始播放", info.title),
        HeroKind::Collection => format!("播放歌单「{}」", info.title),
    };

    div()
        .id("hero-play")
        .flex_none()
        .size(px(PLAY_BTN))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme::control())
        .cursor_pointer()
        // 这版 gpui 的元素没有 `transform`，悬停放大做不到，只能用阴影。
        .hover(|style| {
            style
                .bg(theme::control_hover())
                .shadow(theme::hover_shadow())
        })
        .tooltip(crate::common::tooltip::builder(label))
        .child(icon(Glyph::Play, 20., theme::text_on_control()))
        .on_click(move |_, _, cx| {
            library.update(cx, |library, cx| {
                // 作用域与搜索自动生效：专辑详情里可见区第一行就是这张专辑的第一首。
                // 第一首若是引擎解不了的格式，会顺延到下一首能播的。
                library.play_first_playable(cx);
            });
        })
        .into_any_element()
}

/// 封面槽：命中就显示图片，否则请求 + 占位。取图走 [`crate::common::cover::cover_state`]。
/// 不能绕开它里的 `is_missing` 那一档，否则没有封面的专辑会每帧重发请求。
fn cover_slot(
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    info: &HeroInfo,
    cx: &mut App,
) -> AnyElement {
    let kind = info.kind;
    let label = info.title.clone();
    let radius = match kind {
        HeroKind::Album | HeroKind::Collection => theme::RADIUS_LG,
        HeroKind::Artist => COVER / 2.0,
    };

    if kind == HeroKind::Collection {
        return crate::common::cover::list_cover(
            covers,
            library,
            COVER,
            radius,
            info.cover_file.as_deref(),
            info.cover_id,
            &label,
            cx,
        );
    }

    let Some(id) = info.cover_id else {
        return placeholder_cell(kind, &label, radius);
    };
    match crate::common::cover::cover_state(covers, library, id, COVER, cx) {
        crate::common::cover::Cover::Ready(image) => {
            crate::common::cover::image_cell(image, COVER, radius)
        }
        crate::common::cover::Cover::Missing => placeholder_cell(kind, &label, radius),
        crate::common::cover::Cover::Pending => div()
            .flex_none()
            .size(px(COVER))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius))
            .bg(theme::elevated())
            .child(crate::common::cover::skeleton(COVER, radius))
            .into_any_element(),
    }
}

/// 专辑 / 艺术家头图「确实没有封面」那一格。
fn placeholder_cell(kind: HeroKind, label: &str, radius: f32) -> AnyElement {
    div()
        .flex_none()
        .size(px(COVER))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius))
        .bg(theme::elevated())
        .border_1()
        .border_color(theme::border())
        .child(placeholder(kind, label.chars().next()))
        .into_any_element()
}

/// 没有封面时的占位：专辑用首字色块（`monogram_color`，各处同一份），艺术家用人形图标。
fn placeholder(kind: HeroKind, initial: Option<char>) -> AnyElement {
    if kind == HeroKind::Artist {
        return div()
            .text_color(theme::text_faint())
            .child(icon_inherit(Glyph::Artist, 64.))
            .into_any_element();
    }

    let Some(ch) = initial else {
        return div()
            .text_color(theme::text_faint())
            .child(icon_inherit(Glyph::Album, 48.))
            .into_any_element();
    };

    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_LG))
        .bg(crate::common::cover::monogram_color(ch))
        .text_color(rgb(0xffffff))
        .text_size(px(64.0))
        .font_weight(theme::WEIGHT_TITLE)
        .child(ch.to_string())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // 不要写 `use super::*`：会把 `crate::platform::*` 里的 gpui `test` 宏带进来，
    // 遮蔽内置 `#[test]`。
    use super::HeroInfo;
    use lyre_core::{Track, TrackId};
    use std::time::Duration;

    /// 造一首曲目；`AlbumGroup` / `ArtistGroup` 的聚合字段私有，只能由真实分组函数得出。
    fn track(id: u64, title: &str, album: &str, artist: &str, secs: Option<u64>) -> Track {
        Track {
            id: TrackId(id),
            path: std::path::PathBuf::from(format!("/music/{id}.mp3")),
            title: title.to_string(),
            artist: Some(artist.to_string()),
            album: Some(album.to_string()),
            duration: secs.map(Duration::from_secs),
            has_cover: false,
            audio: lyre_core::AudioInfo::default(),
        }
    }

    /// 专辑页的「关注」要恰好一位艺术家：多人字段拆开后的那一位才是艺术家页的名字。
    #[test]
    fn album_follow_target_only_exists_for_a_single_artist() {
        let single = lyre_core::group_by_album(&[track(1, "a", "独集", "林一", None)]);
        assert_eq!(
            HeroInfo::from_album(&single[0]).follow_target(),
            Some("林一"),
            "单人专辑仍然要能关注"
        );

        let multi = lyre_core::group_by_album(&[track(2, "b", "合辑", "甲 / 乙", None)]);
        assert_eq!(
            HeroInfo::from_album(&multi[0]).follow_target(),
            None,
            "整串「甲 / 乙」不是任何一位艺术家的名字，关注了也匹配不上"
        );
    }

    #[test]
    fn album_hero_titles_the_album_and_subtitles_the_artist() {
        let tracks = vec![
            track(1, "a", "城市漫游", "林一", None),
            track(2, "b", "城市漫游", "林一", None),
            track(3, "c", "城市漫游", "林一", None),
        ];
        let groups = lyre_core::group_by_album(&tracks);
        let info = HeroInfo::from_album(&groups[0]);

        assert_eq!(info.title, "城市漫游");
        assert_eq!(info.tag, "专辑");
        assert_eq!(info.subtitle, "林一");
        assert_eq!(info.meta, "3 首");
    }

    #[test]
    fn album_hero_reports_duration_when_every_track_is_known() {
        let tracks = vec![
            track(1, "a", "A", "X", Some(60)),
            track(2, "b", "A", "X", Some(65)),
        ];
        let groups = lyre_core::group_by_album(&tracks);
        let info = HeroInfo::from_album(&groups[0]);

        // `format_duration` 的口径是 `M:SS`，分钟不补零。
        assert_eq!(info.meta, "2 首 · 2:05");
    }

    #[test]
    fn album_hero_omits_duration_when_any_track_is_unknown() {
        let tracks = vec![
            track(1, "a", "A", "X", Some(60)),
            track(2, "b", "A", "X", None),
        ];
        let groups = lyre_core::group_by_album(&tracks);
        let info = HeroInfo::from_album(&groups[0]);

        assert_eq!(info.meta, "2 首");
    }

    #[test]
    fn album_hero_with_multiple_artists_says_various() {
        let tracks = vec![
            track(1, "a", "合辑", "A", None),
            track(2, "b", "合辑", "B", None),
        ];
        let groups = lyre_core::group_by_album(&tracks);
        let info = HeroInfo::from_album(&groups[0]);

        assert_eq!(info.subtitle, lyre_core::VARIOUS_ARTISTS);
    }

    #[test]
    fn artist_hero_subtitles_the_album_count() {
        let tracks = vec![
            track(1, "a", "Lo-Fi Beats", "Chill Studio", None),
            track(2, "b", "Night Drive", "Chill Studio", None),
        ];
        let groups = lyre_core::group_by_artist(&tracks);
        let info = HeroInfo::from_artist(&groups[0]);

        assert_eq!(info.title, "Chill Studio");
        assert_eq!(info.tag, "艺术家");
        assert_eq!(info.subtitle, "2 张专辑");
        assert_eq!(info.meta, "2 首");
    }

    #[test]
    fn title_is_never_blank() {
        // 分组保证标题非空：无标签的曲目归「未知专辑」。
        let tracks = vec![track(1, "a", "", "", None)];
        let groups = lyre_core::group_by_album(&tracks);
        let info = HeroInfo::from_album(&groups[0]);

        assert!(!info.title.trim().is_empty());
    }

    #[test]
    fn artist_hero_follows_the_artist_itself() {
        let tracks = vec![track(1, "a", "Lo-Fi Beats", "Chill Studio", None)];
        let info = HeroInfo::from_artist(&lyre_core::group_by_artist(&tracks)[0]);
        assert_eq!(info.follow_target(), Some("Chill Studio"));
    }

    #[test]
    fn album_hero_follows_its_single_artist() {
        let tracks = vec![track(1, "a", "城市漫游", "林一", None)];
        let info = HeroInfo::from_album(&lyre_core::group_by_album(&tracks)[0]);
        assert_eq!(info.follow_target(), Some("林一"));
    }

    #[test]
    fn album_with_several_artists_has_no_follow_target() {
        let tracks = vec![
            track(1, "a", "合辑", "A", None),
            track(2, "b", "合辑", "B", None),
        ];
        let info = HeroInfo::from_album(&lyre_core::group_by_album(&tracks)[0]);
        assert_eq!(info.follow_target(), None);
    }

    #[test]
    fn album_without_artist_has_no_follow_target() {
        let tracks = vec![track(1, "a", "城市漫游", "", None)];
        let info = HeroInfo::from_album(&lyre_core::group_by_album(&tracks)[0]);
        assert_eq!(info.follow_target(), None, "空艺术家不该给出关注对象");
    }
}
