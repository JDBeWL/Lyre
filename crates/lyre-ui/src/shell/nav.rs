//! 主区导航：区段、歌词遮罩、前进 / 后退历史栈。

use crate::entities::Scope;
use crate::platform::*;

use super::ShellEntity;

/// 主区正在显示哪个区段。没有「专辑详情」区段：进入专辑只是把 `LibraryEntity` 的作用域收窄成那张专辑，主区仍是 `Tracks`。
/// `Home` / `Search` 由侧栏驱动、参与导航历史，但不带作用域渲染。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum NavSection {
    /// 最近播放 + 快捷入口
    Home,
    /// 平铺曲目
    #[default]
    Tracks,
    /// 按专辑分组的封面网格
    Albums,
    /// 按艺术家分组的封面网格
    Artists,
    /// 搜索页：大输入框 + 结果
    Search,
    /// 设置页：顶栏齿轮与 `Ctrl + ,` 的唯一去处
    Settings,
}

impl NavSection {
    /// 出现在主区区段切换器里的那三个，不含 `Home` / `Search`。
    pub const LIBRARY: [NavSection; 3] = [Self::Tracks, Self::Albums, Self::Artists];

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "首页",
            Self::Tracks => "曲目",
            Self::Albums => "专辑",
            Self::Artists => "艺术家",
            Self::Search => "搜索",
            Self::Settings => "设置",
        }
    }

    /// 侧栏导航项用的图标。
    pub fn glyph(self) -> crate::icon::Glyph {
        match self {
            Self::Home => crate::icon::Glyph::Home,
            Self::Tracks => crate::icon::Glyph::List,
            Self::Albums => crate::icon::Glyph::Album,
            Self::Artists => crate::icon::Glyph::Artist,
            Self::Search => crate::icon::Glyph::Search,
            Self::Settings => crate::icon::Glyph::Settings,
        }
    }

    /// 这个区段是不是「曲库的一个切面」（`Tracks` / `Albums` / `Artists`）。
    pub fn in_library(self) -> bool {
        matches!(self, Self::Tracks | Self::Albums | Self::Artists)
    }
}

impl ShellEntity {
    pub fn nav_section(&self) -> NavSection {
        self.nav
    }

    /// 歌词全屏遮罩是否展开。它盖在主区卡片上、不改变布局，所以不牵连网格列数。
    pub fn shows_lyrics(&self) -> bool {
        self.lyrics
    }

    /// 切换主区导航区段。重复选中当前区段时不重绘。
    pub fn set_nav(&mut self, nav: NavSection, cx: &mut Context<Self>) {
        if self.apply_nav(nav) {
            cx.notify();
        }
    }

    /// `set_nav` 的状态变更本体，不 notify。
    /// 换区段顺带收起歌词遮罩 —— 它盖住整块主区，留着会挡住新区段。
    fn apply_nav(&mut self, nav: NavSection) -> bool {
        if self.nav == nav {
            return false;
        }
        self.nav = nav;
        self.lyrics = false;
        true
    }

    pub fn toggle_lyrics(&mut self, cx: &mut Context<Self>) {
        self.lyrics = !self.lyrics;
        cx.notify();
    }

    /// 收起歌词遮罩，已经收着就不重绘。主区换了内容都要调它（`apply_nav`、`LibraryEvent::ScopeChanged`）。
    pub fn close_lyrics(&mut self, cx: &mut Context<Self>) {
        if self.lyrics {
            self.lyrics = false;
            cx.notify();
        }
    }

    // 这组只维护栈本身；落点还原与作用域恢复由 `root::nav::navigate` 负责。

    /// 主区现在能不能往回退。
    pub fn can_go_back(&self) -> bool {
        !self.nav_history.is_empty()
    }

    /// 看一眼后退落点，不弹出。悬停提示要问这里而不是当前作用域，否则文案是反的。
    pub fn peek_history(&self) -> Option<(NavSection, Scope)> {
        self.nav_history.last().cloned()
    }

    /// 主区现在能不能往前走（曾经退过、且没被新导航作废）。
    pub fn can_go_forward(&self) -> bool {
        !self.nav_forward.is_empty()
    }

    /// 压入一个可返回的落点，相邻重复折叠；顺带清空前进栈，不分给调用方。
    /// 进入详情时调，返回时不要调。
    pub fn push_history(&mut self, section: NavSection, scope: Scope, cx: &mut Context<Self>) {
        self.nav_forward.clear();
        if self.nav_history.last() == Some(&(section, scope.clone())) {
            return;
        }
        self.nav_history.push((section, scope));
        cx.notify();
    }

    /// 弹出一个落点供「后退」使用；参数是当前位置，要记进前进栈。空栈返回 `None`。
    /// 刻意不 `cx.notify()`：重绘由 `RootView::go_back` 在曲库实体的上下文里触发，分两处会渲染两遍。
    pub fn pop_history(&mut self, current: (NavSection, Scope)) -> Option<(NavSection, Scope)> {
        let popped = self.nav_history.pop()?;
        self.nav_forward.push(current);
        Some(popped)
    }

    /// 弹出一个「前进」落点，并把当前落点压回后退栈；与 [`pop_history`](Self::pop_history) 严格对称。
    pub fn pop_forward(&mut self, current: (NavSection, Scope)) -> Option<(NavSection, Scope)> {
        let popped = self.nav_forward.pop()?;
        self.nav_history.push(current);
        Some(popped)
    }

    /// 清空两侧历史：点侧栏导航项是一次平级跳跃，两条栈都不作数。
    pub fn clear_history(&mut self) {
        self.nav_history.clear();
        self.nav_forward.clear();
    }
}

/// 从任何入口进一个作用域：清搜索词 → 收窄 → 切到曲库页。搜索词属于离开的那一屏，带着它只会得到空结果。
/// 历史由调用方按入口性质决定 `push_history` 还是 `clear_history`；切页不能省，`AlbumGrid` 不读 `Scope`。
pub fn enter_scope(
    scope: Scope,
    shell: &Entity<ShellEntity>,
    library: &Entity<crate::entities::LibraryEntity>,
    cx: &mut App,
) {
    library.update(cx, |library, cx| {
        library.clear_query(cx);
        match scope {
            Scope::Album(title) => library.enter_album(title, cx),
            Scope::Artist(name) => library.enter_artist(name, cx),
            Scope::Collection(id) => library.enter_collection(id, cx),
            Scope::Favorites => library.enter_favorites(cx),
            // 「整个曲库」不是一个可进入的落点：那是退出，走 `exit_scope`
            Scope::All => {}
        }
    });
    shell.update(cx, |shell, cx| shell.set_nav(NavSection::Tracks, cx));
}

/// 跳到某个区段（首页 / 曲库 / 专辑 / 艺术家）并退出作用域，先记下当前位置供顶栏 ← 退回。
/// 记的是**离开前**的 (区段, 作用域)：退出作用域后 `scope()` 已是 `All`，那时再记就记错。
pub fn go_to(
    section: NavSection,
    shell: &Entity<ShellEntity>,
    library: &Entity<crate::entities::LibraryEntity>,
    cx: &mut App,
) {
    let here = (
        shell.read(cx).nav_section(),
        library.read(cx).scope().clone(),
    );
    library.update(cx, |library, cx| {
        if library.scope().is_scoped() {
            library.exit_scope(cx);
        }
    });
    shell.update(cx, |shell, cx| {
        shell.push_history(here.0, here.1, cx);
        shell.set_nav(section, cx);
    });
}

/// 从别处进一个作用域：先记下当前位置供顶栏 ← 退回，再走 [`enter_scope`]。
pub fn enter_scope_from_here(
    scope: Scope,
    shell: &Entity<ShellEntity>,
    library: &Entity<crate::entities::LibraryEntity>,
    cx: &mut App,
) {
    let here = (
        shell.read(cx).nav_section(),
        library.read(cx).scope().clone(),
    );
    shell.update(cx, |shell, cx| shell.push_history(here.0, here.1, cx));
    enter_scope(scope, shell, library, cx);
}

#[cfg(test)]
mod tests {
    use super::{NavSection, ShellEntity};
    use crate::entities::Scope;
    use crate::shell::{Breakpoint, Density};

    /// 不启动窗口订阅的实例；`ShellEntity::new` 需要 `&mut Window`。
    fn shell_for_history() -> ShellEntity {
        use crate::platform::Subscription;
        ShellEntity {
            width: 1600.0,
            breakpoint: Breakpoint::for_width(1600.0),
            density: Density::Compact,
            sidebar_override: None,
            right_panel: None,
            lyrics: false,
            nav: NavSection::default(),
            nav_history: Vec::new(),
            nav_forward: Vec::new(),
            grid_columns: 4,
            grid_card_width: 200.0,
            _bounds: Subscription::new(|| {}),
        }
    }

    /// 复现 `push_history` 的两条规则：新导航清空前进栈、栈顶相同就不压。
    /// 不直接调它：造 `Context` 要 gpui 的 `test-support` feature，而对依赖项不生效。
    fn push(shell: &mut ShellEntity, section: NavSection, scope: Scope) {
        let snapshot = (section, scope);
        shell.nav_forward.clear();
        if shell.nav_history.last() != Some(&snapshot) {
            shell.nav_history.push(snapshot);
        }
    }

    /// 「当前位置」元组，供 `pop_history` / `pop_forward` 记进对面那条栈。
    fn here(section: NavSection, scope: Scope) -> (NavSection, Scope) {
        (section, scope)
    }

    #[test]
    fn changing_section_folds_the_lyrics_overlay() {
        let mut shell = shell_for_history();
        shell.apply_nav(NavSection::Home);
        shell.lyrics = true;

        assert!(shell.apply_nav(NavSection::Albums), "区段变了要重绘");
        assert!(!shell.shows_lyrics(), "换区段必须收起遮罩");
    }

    #[test]
    fn reselecting_the_current_section_needs_no_repaint() {
        let mut shell = shell_for_history();
        shell.apply_nav(NavSection::Albums);
        assert!(
            !shell.apply_nav(NavSection::Albums),
            "重复选中当前区段不该重绘"
        );
    }

    #[test]
    fn empty_history_means_no_back_button() {
        let shell = shell_for_history();
        assert!(!shell.can_go_back());
        assert!(!shell.can_go_forward());
    }

    #[test]
    fn history_pops_in_reverse_order() {
        let mut shell = shell_for_history();
        push(&mut shell, NavSection::Albums, Scope::All);
        push(
            &mut shell,
            NavSection::Tracks,
            Scope::Album("城市漫游".to_owned()),
        );
        assert!(shell.can_go_back());

        assert_eq!(
            shell.pop_history(here(NavSection::Artists, Scope::Album("某某".to_owned()))),
            Some((NavSection::Tracks, Scope::Album("城市漫游".to_owned())))
        );
        assert_eq!(
            shell.pop_history(here(NavSection::Albums, Scope::All)),
            Some((NavSection::Albums, Scope::All))
        );
        assert_eq!(
            shell.pop_history(here(NavSection::Albums, Scope::All)),
            None
        );
        assert!(!shell.can_go_back());
    }

    #[test]
    fn clear_history_empties_the_stack() {
        let mut shell = shell_for_history();
        push(&mut shell, NavSection::Albums, Scope::All);
        push(
            &mut shell,
            NavSection::Tracks,
            Scope::Album("城市漫游".to_owned()),
        );
        shell.clear_history();
        assert!(!shell.can_go_back());
        assert_eq!(
            shell.pop_history(here(NavSection::Tracks, Scope::All)),
            None
        );
    }

    #[test]
    fn pushing_the_same_spot_twice_is_collapsed() {
        let mut shell = shell_for_history();
        push(&mut shell, NavSection::Albums, Scope::All);
        push(&mut shell, NavSection::Albums, Scope::All);
        assert_eq!(
            shell.pop_history(here(NavSection::Tracks, Scope::All)),
            Some((NavSection::Albums, Scope::All))
        );
        assert_eq!(
            shell.pop_history(here(NavSection::Tracks, Scope::All)),
            None,
            "重复的落点必须被折叠，栈里只该有一条"
        );
    }

    #[test]
    fn different_spots_are_both_kept() {
        // 去重只针对相邻重复：往返两处是两次独立导航，都要留着
        let mut shell = shell_for_history();
        push(&mut shell, NavSection::Albums, Scope::All);
        push(&mut shell, NavSection::Tracks, Scope::Album("A".to_owned()));
        push(&mut shell, NavSection::Albums, Scope::All);
        assert_eq!(
            shell.pop_history(here(NavSection::Tracks, Scope::All)),
            Some((NavSection::Albums, Scope::All))
        );
        assert_eq!(
            shell.pop_history(here(NavSection::Tracks, Scope::All)),
            Some((NavSection::Tracks, Scope::Album("A".to_owned())))
        );
        assert_eq!(
            shell.pop_history(here(NavSection::Tracks, Scope::All)),
            Some((NavSection::Albums, Scope::All))
        );
    }

    #[test]
    fn backing_up_then_going_forward_returns_to_the_start() {
        let mut shell = shell_for_history();
        push(&mut shell, NavSection::Albums, Scope::All);

        let back = shell.pop_history(here(
            NavSection::Tracks,
            Scope::Album("城市漫游".to_owned()),
        ));
        assert_eq!(back, Some((NavSection::Albums, Scope::All)));
        assert!(shell.can_go_forward());
        assert!(!shell.can_go_back());

        let forward = shell.pop_forward(here(NavSection::Albums, Scope::All));
        assert_eq!(
            forward,
            Some((NavSection::Tracks, Scope::Album("城市漫游".to_owned()))),
            "前进必须回到后退前所在的那个详情页"
        );
        assert!(shell.can_go_back());
        assert!(!shell.can_go_forward());
    }

    #[test]
    fn two_backs_support_two_forwards() {
        let mut shell = shell_for_history();
        push(&mut shell, NavSection::Albums, Scope::All);
        push(&mut shell, NavSection::Tracks, Scope::Album("A".to_owned()));

        assert_eq!(
            shell.pop_history(here(NavSection::Tracks, Scope::Album("B".to_owned()))),
            Some((NavSection::Tracks, Scope::Album("A".to_owned())))
        );
        assert_eq!(
            shell.pop_history(here(NavSection::Tracks, Scope::Album("A".to_owned()))),
            Some((NavSection::Albums, Scope::All))
        );
        assert!(!shell.can_go_back());

        assert_eq!(
            shell.pop_forward(here(NavSection::Albums, Scope::All)),
            Some((NavSection::Tracks, Scope::Album("A".to_owned())))
        );
        assert_eq!(
            shell.pop_forward(here(NavSection::Tracks, Scope::Album("A".to_owned()))),
            Some((NavSection::Tracks, Scope::Album("B".to_owned())))
        );
        assert!(!shell.can_go_forward());
    }

    #[test]
    fn a_new_navigation_drops_the_forward_stack() {
        let mut shell = shell_for_history();
        push(&mut shell, NavSection::Albums, Scope::All);
        push(&mut shell, NavSection::Tracks, Scope::Album("A".to_owned()));

        shell.pop_history(here(NavSection::Tracks, Scope::Album("A".to_owned())));
        assert!(shell.can_go_forward());

        push(
            &mut shell,
            NavSection::Artists,
            Scope::Album("某某".to_owned()),
        );
        assert!(
            !shell.can_go_forward(),
            "新的导航必须清空前进栈，否则前进会跳到一个用户没去过的分支"
        );
    }

    #[test]
    fn forward_stack_is_also_cleared_by_an_explicit_jump() {
        let mut shell = shell_for_history();
        push(&mut shell, NavSection::Albums, Scope::All);
        shell.pop_history(here(NavSection::Tracks, Scope::Album("A".to_owned())));
        assert!(shell.can_go_forward());

        shell.clear_history();
        assert!(!shell.can_go_back());
        assert!(!shell.can_go_forward());
        assert_eq!(
            shell.pop_forward(here(NavSection::Albums, Scope::All)),
            None
        );
    }
}
