//! 侧边栏：上卡片是首页 / 曲库两个导航项，下卡片列曲库内容。
//! 下卡片粒度默认跟随主区区段（`entries::listing_for`），可被标题下的胶囊 Chip 覆盖（`listing_override`）。

use crate::cover::CoverCache;
use crate::entities::LibraryEntity;
use crate::platform::*;
use crate::shell::{NavSection, ShellEntity};
use crate::theme;

mod card;
mod entries;
mod menu;
mod nav;
mod thumb;

/// 歌单的右键菜单。
pub(crate) use menu::collection_menu;

use entries::Listing;

pub struct Sidebar {
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
    covers: Entity<CoverCache>,
    /// Chip 手动筛选；`None` = 跟随主区区段（`entries::listing_for`）。
    listing_override: Option<Listing>,
    /// 上次渲染时的主区区段：区段一变就把手动筛选复位回「跟随」，
    /// 否则从「专辑」切到「艺术家」区段时侧栏还停在专辑列表。
    last_nav: NavSection,
    _subs: Vec<Subscription>,
}

impl Sidebar {
    pub fn new(
        shell: Entity<ShellEntity>,
        library: Entity<LibraryEntity>,
        covers: Entity<CoverCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe_shell = cx.observe(&shell, |_, _, cx| cx.notify());
        let observe_library = cx.observe(&library, |_, _, cx| cx.notify());
        // 封面解码完成时要重绘，否则缩略图一直停在占位图上。
        let observe_covers = cx.observe(&covers, |_, _, cx| cx.notify());

        Self {
            shell,
            library,
            covers,
            listing_override: None,
            last_nav: NavSection::Home,
            _subs: vec![observe_shell, observe_library, observe_covers],
        }
    }

    /// 下卡片该列什么：手动筛选优先，否则跟随主区区段。
    fn effective_listing(&self, nav: NavSection) -> Listing {
        self.listing_override
            .unwrap_or_else(|| entries::listing_for(nav))
    }

    /// Chip 点击：切到指定粒度，覆盖「跟随主区」。
    fn set_listing(&mut self, listing: Listing, cx: &mut Context<Self>) {
        if self.listing_override != Some(listing) {
            self.listing_override = Some(listing);
            cx.notify();
        }
    }

    /// 每个导航项右侧的计数。`albums()` / `artists()` 惰性分组，要用 `update` 才触发重算；
    /// `Entity::update` 不自动 `notify`，所以在 render 里这样用不会形成重绘循环。
    fn counts(&self, cx: &mut App) -> (usize, usize, usize) {
        App::update_entity(cx, &self.library, |library, _| {
            (
                library.len(),
                library.albums().len(),
                library.artists().len(),
            )
        })
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.shell.read(cx).sidebar();
        let nav = self.shell.read(cx).nav_section();
        // 主区区段变了：把手动筛选复位回「跟随主区」。在 render 里改而不 notify —— 本帧就会用上新值。
        if self.last_nav != nav {
            self.last_nav = nav;
            self.listing_override = None;
        }
        let counts = self.counts(cx);

        if !state.shows_labels() {
            return self.render_icons(state, counts, cx);
        }

        div()
            .flex_none()
            .w(px(state.width()))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(theme::GUTTER))
            .child(self.home_card(state, cx))
            .child(self.library_card(state, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`crate::platform::*` 会把 gpui 的 `test` 宏带进来，遮蔽内置 `#[test]`。
    use super::entries::{Listing, SideScope, listing_for};
    use super::nav::library_item_selected;
    use crate::shell::NavSection;

    /// 「新建歌单」那一行没有现成的作用域可进 —— 它得先把歌单建出来。
    #[test]
    fn only_the_new_row_has_no_scope_to_enter() {
        use crate::entities::Scope;
        use lyre_core::CollectionId;

        assert_eq!(
            SideScope::Album.scope_of("某张专辑"),
            Some(Scope::Album("某张专辑".to_owned()))
        );
        assert_eq!(
            SideScope::Artist.scope_of("某人"),
            Some(Scope::Artist("某人".to_owned()))
        );
        assert_eq!(
            SideScope::Collection(CollectionId(3)).scope_of("随便什么标题"),
            Some(Scope::Collection(CollectionId(3))),
            "歌单进哪一个只看 id，行标题说了不算"
        );
        assert_eq!(
            SideScope::Favorites.scope_of("我喜欢的音乐"),
            Some(Scope::Favorites)
        );
        assert_eq!(SideScope::NewCollection.scope_of("新歌单 1"), None);
    }

    #[test]
    fn tracks_section_lists_artists_not_albums() {
        // 改回专辑不会报错，只会侧栏和主区对不上。
        assert_eq!(
            listing_for(NavSection::Tracks),
            Listing::Artists,
            "曲目区段必须列艺术家 —— 主区是平铺曲目，列专辑会让两边对不上"
        );
    }

    #[test]
    fn grid_sections_list_what_their_grid_shows() {
        assert_eq!(listing_for(NavSection::Albums), Listing::Albums);
        assert_eq!(listing_for(NavSection::Artists), Listing::Artists);
    }

    #[test]
    fn every_section_has_a_listing() {
        // 显式列出五个变体：有「全部」常量时，新增忘了加进数组就检不出来。
        let all = [
            NavSection::Home,
            NavSection::Tracks,
            NavSection::Albums,
            NavSection::Artists,
            NavSection::Search,
        ];
        for nav in all {
            let listing = listing_for(nav);
            assert!(
                matches!(listing, Listing::Albums | Listing::Artists),
                "区段 {nav:?} 应当能映射到某种列表"
            );
        }
    }

    #[test]
    fn library_item_lights_up_only_at_the_library_root() {
        for nav in [NavSection::Tracks, NavSection::Albums, NavSection::Artists] {
            assert!(
                library_item_selected(nav, false),
                "在{nav:?}区段（未进详情）时「曲库」应当点亮"
            );
        }

        assert!(
            !library_item_selected(NavSection::Tracks, true),
            "进了详情（有作用域）时「曲库」不该再亮 —— 亮的是列表里那一行"
        );

        for nav in [NavSection::Home, NavSection::Search] {
            assert!(
                !library_item_selected(nav, false),
                "{nav:?} 不是曲库的切面，不该点亮「曲库」"
            );
        }
    }
}
