//! 左键展开的下拉菜单触发器：`ContextMenuExt::context_menu` 只响应右键，`DropdownMenu` 又要求触发器实现 `Selectable`。

use std::rc::Rc;

use lyre_core::{Track, TrackId, UNKNOWN_ALBUM, UNKNOWN_ARTIST};

use crate::entities::LibraryEntity;
use crate::platform::*;
use crate::theme;

pub struct MenuTrigger {
    element: Stateful<Div>,
    /// 菜单是否正开着，由库里的 `Popover` 通过 [`Selectable::open`] 写入
    open: bool,
}

impl MenuTrigger {
    pub fn new(element: Stateful<Div>) -> Self {
        Self {
            element,
            open: false,
        }
    }
}

impl Styled for MenuTrigger {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl InteractiveElement for MenuTrigger {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl Selectable for MenuTrigger {
    fn selected(mut self, selected: bool) -> Self {
        // `Selectable::open` 的默认实现转发到 `selected`，接一次就够
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl IntoElement for MenuTrigger {
    type Element = <Stateful<Div> as IntoElement>::Element;

    fn into_element(self) -> Self::Element {
        let mut element = self.element;
        if self.open {
            element = element.bg(theme::elevated()).text_color(theme::text());
        }
        element.into_element()
    }
}

impl DropdownMenu for MenuTrigger {}

/// 左键点击展开的菜单。`anchor` 决定弹出边；贴区域右缘的入口用右对齐，避免菜单探出窗口。
pub fn dropdown(
    trigger: Stateful<Div>,
    anchor: Anchor,
    build: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> AnyElement {
    MenuTrigger::new(trigger)
        .dropdown_menu_with_anchor(anchor, build)
        .into_any_element()
}

/// 菜单项文字的最宽值。专辑名 / 艺术家名可以长到几百像素，而 `PopupMenu` 的容器只有
/// `max_w`、不裁横向溢出，所以宽度必须在文字这一层收。
const LABEL_MAX_W: f32 = 320.0;

/// 一条文字可能很长的菜单项：限最宽 + 省略号，不是固定宽。
/// 不加 tooltip：`.tooltip()` 要求元素有 id，而重名歌单 / 专辑会让两项撞同一个 id。
pub fn long_item(text: impl Into<SharedString>) -> PopupMenuItem {
    let text = text.into();
    PopupMenuItem::element(move |_window, _cx| {
        div().max_w(px(LABEL_MAX_W)).truncate().child(text.clone())
    })
}

/// 多人字段只给第一位一个入口，菜单文案要把「还有别人」写进名字。
pub fn artist_menu_label(first: &str, total: usize) -> String {
    if total > 1 {
        format!("查看艺术家 {first} 等 {total} 人")
    } else {
        format!("查看艺术家 {first}")
    }
}

/// 「进入专辑 / 艺术家之前」的副作用。
type BeforeLeave = Rc<dyn Fn(&mut App)>;
type AfterRemove = Rc<dyn Fn(Track, usize, &mut App)>;
/// 「打开这个分组」的落点。
type OnOpen = Rc<dyn Fn(&mut App)>;
/// 「从这一首开始播」的落点。
type OnPlay = Rc<dyn Fn(TrackId, &mut App)>;

/// 菜单动作在宿主侧引起的副作用：曲库行要进专辑 / 艺术家前记住滚动落点、移除后播一行动画；
/// 队列行与搜索结果行用 [`TrackMenuHooks::none`]。
#[derive(Clone, Default)]
pub struct TrackMenuHooks {
    play: Option<OnPlay>,
    before_leave: Option<BeforeLeave>,
    after_remove: Option<AfterRemove>,
}

impl TrackMenuHooks {
    /// 没有额外副作用：搜索结果行的用法。
    pub fn none() -> Self {
        Self::default()
    }

    /// 换掉「播放」的落点，默认是列表规则（见 [`track_context_menu`]）。
    /// 队列行必须换：走 `activate_in_view` 会把队列重设成曲库顺序，随机模式当场失效。
    pub fn play(mut self, hook: impl Fn(TrackId, &mut App) + 'static) -> Self {
        self.play = Some(Rc::new(hook));
        self
    }

    /// 进专辑 / 艺术家页之前触发。曲库行用它记下落点，否则返回时列表跳回开头。
    pub fn before_leave(mut self, hook: impl Fn(&mut App) + 'static) -> Self {
        self.before_leave = Some(Rc::new(hook));
        self
    }

    /// 从曲库移除之后触发，参数是被移除的曲目与它原来的位置。曲库行用它播移除动画。
    pub fn after_remove(mut self, hook: impl Fn(Track, usize, &mut App) + 'static) -> Self {
        self.after_remove = Some(Rc::new(hook));
        self
    }

    fn leaving(&self, cx: &mut App) {
        if let Some(hook) = &self.before_leave {
            hook(cx);
        }
    }

    fn removed(&self, track: Track, at: usize, cx: &mut App) {
        if let Some(hook) = &self.after_remove {
            hook(track, at, cx);
        }
    }
}

/// 一首曲目的右键菜单。「加入歌单」的勾选态就是成员关系：勾着的再点一次是移出，不另设「从歌单移除」。
pub fn track_context_menu(
    library: Entity<LibraryEntity>,
    track_id: TrackId,
    hooks: TrackMenuHooks,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _window, cx| {
        let mut menu = menu;

        // 专辑 / 艺术家名拖到「菜单真的被打开」这一刻才取，避免每行每次重绘都克隆两个 String。
        let (album, artist) = {
            let read = library.read(cx);
            let track = read.playlist().by_id(track_id);
            (
                track.and_then(|track| track.album.clone()),
                track.and_then(|track| track.artist.clone()),
            )
        };

        {
            let library = library.clone();
            let play = hooks.play.clone();
            menu = menu.item(PopupMenuItem::new("播放").on_click(move |_, _, cx| {
                if let Some(hook) = &play {
                    hook(track_id, cx);
                    return;
                }
                // 默认沿用列表规则：队列跟着你正在看的列表走（见 `activate_in_view`）。
                library.update(cx, |library, cx| {
                    library.activate_in_view(track_id, cx);
                });
            }));
        }

        // 用 lyre-core 的分组键判定而不是自己写 `is_empty`：分组、下钻筛选、菜单三处规则必须一致，
        // 否则会出现「菜单里能进这张专辑，进去却一首都没有」。
        let has_album = lyre_core::album_key_str(album.as_deref()) != UNKNOWN_ALBUM;
        // 多人字段只给第一位一个入口（其余人从艺术家页 / 搜索进）：整串「甲/乙」在分组里不存在。
        let artists: Vec<&str> = lyre_core::artist_keys_str(artist.as_deref())
            .into_iter()
            .filter(|key| *key != UNKNOWN_ARTIST)
            .collect();

        if has_album || !artists.is_empty() {
            menu = menu.item(PopupMenuItem::separator());
        }

        // 闭包是 `Fn`，不能从捕获里 move 出去，每次都克隆一份进各自的 `on_click`。
        if let Some(title) = album.clone()
            && has_album
        {
            let library = library.clone();
            let hooks = hooks.clone();
            menu = menu.item(long_item(format!("查看专辑《{title}》")).on_click(
                move |_, _, cx| {
                    // 先报落点再收窄作用域，顺序反了会记成「进入之后」。
                    hooks.leaving(cx);
                    library.update(cx, |library, cx| library.enter_album(title.clone(), cx));
                },
            ));
        }

        if let Some(first) = artists.first() {
            let name = (*first).to_owned();
            let label = artist_menu_label(&name, artists.len());
            let library = library.clone();
            let hooks = hooks.clone();
            menu = menu.item(long_item(label).on_click(move |_, _, cx| {
                hooks.leaving(cx);
                library.update(cx, |library, cx| library.enter_artist(name.clone(), cx));
            }));
        }

        let lists: Vec<(lyre_core::CollectionId, String, bool)> = {
            let library = library.read(cx);
            library
                .collections()
                .iter()
                .map(|list| (list.id, list.name.clone(), list.contains(track_id)))
                .collect()
        };
        if !lists.is_empty() {
            menu = menu.item(PopupMenuItem::separator());
        }
        for (id, name, inside) in lists {
            let library = library.clone();
            let mark = if inside { "✓ " } else { "" };
            menu = menu.item(long_item(format!("{mark}加入「{name}」")).on_click(
                move |_, _, cx| {
                    library.update(cx, |library, cx| {
                        if inside {
                            library.remove_from_collection(id, track_id, cx);
                        } else {
                            library.add_to_collection(id, track_id, cx);
                        }
                    });
                },
            ));
        }
        {
            let library = library.clone();
            menu = menu.item(
                PopupMenuItem::new("新建歌单并加入").on_click(move |_, _, cx| {
                    library.update(cx, |library, cx| {
                        // 名字带序号：连着建两个而不改名的话，同名会让侧栏两行看起来一样
                        let name = format!("新歌单 {}", library.collections().len() + 1);
                        if let Some(id) = library.create_collection(&name, cx) {
                            library.add_to_collection(id, track_id, cx);
                        }
                    });
                }),
            );
        }

        {
            let library = library.clone();
            let hooks = hooks.clone();
            menu = menu.item(PopupMenuItem::separator()).item(
                PopupMenuItem::new("从曲库移除").on_click(move |_, _, cx| {
                    let removed = library.update(cx, |library, cx| library.remove(track_id, cx));
                    if let Some((track, at)) = removed {
                        hooks.removed(track, at, cx);
                    }
                }),
            );
        }

        menu
    }
}

/// 分组卡（专辑 / 艺术家）代表的东西。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    Album,
    Artist,
}

/// 一个分组的右键菜单。差别只有「打开」的落点，由调用方以 `on_open` 传入。
pub fn group_context_menu(
    library: Entity<LibraryEntity>,
    kind: GroupKind,
    id: TrackId,
    title: String,
    on_open: OnOpen,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _window, _cx| {
        let mut menu = menu;

        {
            let on_open = Rc::clone(&on_open);
            let label = match kind {
                GroupKind::Album => format!("打开专辑《{title}》"),
                GroupKind::Artist => format!("打开艺术家 {title}"),
            };
            menu = menu.item(long_item(label).on_click(move |_, _, cx| on_open(cx)));
        }

        {
            let library = library.clone();
            let title = title.clone();
            menu = menu.item(PopupMenuItem::new("播放").on_click(move |_, _, cx| {
                library.update(cx, |library, cx| {
                    // 卡片代表的就是这张专辑（或这位艺术家），队列跟它走。
                    match kind {
                        GroupKind::Album => library.activate_album(&title, id, cx),
                        GroupKind::Artist => library.activate_artist(&title, id, cx),
                    };
                });
            }));
        }

        menu
    }
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`platform::*` 里的 gpui `test` 过程宏会遮蔽内置 `#[test]`。
    use super::artist_menu_label;

    #[test]
    fn a_collaboration_says_so_on_the_artist_menu_item() {
        assert_eq!(artist_menu_label("陈屿", 1), "查看艺术家 陈屿");
        assert_eq!(artist_menu_label("23.exe", 2), "查看艺术家 23.exe 等 2 人");
    }
}
