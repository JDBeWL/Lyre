//! 窗口外壳状态：主区区段、侧栏形态、右面板开合、网格列数。
//! 窗口尺寸变化走 `Context::observe_window_bounds` 重算。

mod layout;
mod nav;

use crate::entities::Scope;
use crate::platform::*;

pub use layout::{
    Breakpoint, CARD_MAX_W, CARD_MIN_W, Density, MAX_GRID_COLUMNS, MEDIUM_MIN, STACKED_BOTTOM_MIN,
    SidebarState, WIDE_MIN,
};
pub use nav::{NavSection, enter_scope, enter_scope_from_here, go_to};

/// 右侧面板的页签。`None` 表示整个面板收起。
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum PanelTab {
    NowPlaying,
    Queue,
}

pub struct ShellEntity {
    width: f32,
    breakpoint: Breakpoint,
    density: Density,
    /// 用户手动调过的侧栏形态，`None` 表示从未调过（[`Self::toggle_sidebar`] 写它）。
    /// 生效形态取它与断点默认里更收起的那个（见 [`Self::sidebar`]），所以尺寸变化不改写它。
    sidebar_override: Option<SidebarState>,
    right_panel: Option<PanelTab>,
    /// 歌词全屏遮罩。主区换了内容必须收它，否则新内容被盖住。
    lyrics: bool,
    /// 主区导航区段，由侧栏驱动。
    nav: NavSection,
    /// 导航历史栈，非空就画返回按钮。存整个 [`Scope`]，有效性判断在 `restore_scope`。
    nav_history: Vec<(NavSection, Scope)>,
    /// 「后退」弹出的落点，供「前进」取用；任何新导航都会清空它。
    nav_forward: Vec<(NavSection, Scope)>,
    /// 专辑网格每行几列，按可用宽度反推：卡宽由 `flex_1` 平分、封面高度恒定，钉死列数会拉扁卡片。
    grid_columns: usize,
    /// 单卡宽度，与 `grid_columns` 同源算出（两者都出自 `grid_metrics`）。
    grid_card_width: f32,
    /// 窗口尺寸变化订阅。`observe_window_bounds` 不会立刻回调一次，
    /// 所以初值仍必须由 `new()` 自己播种。
    _bounds: Subscription,
}

impl ShellEntity {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let width = f32::from(window.viewport_size().width);
        let breakpoint = Breakpoint::for_width(width);

        let bounds = cx.observe_window_bounds(window, |shell, window, cx| {
            shell.apply_viewport(f32::from(window.viewport_size().width), cx);
        });

        let right_panel = if breakpoint.right_panel_expanded() {
            Some(PanelTab::NowPlaying)
        } else {
            None
        };

        // 与 `recompute_grid_columns` 同一算式；此刻还没有 override，用断点默认的侧栏形态。
        let (grid_columns, grid_card_width) = {
            let panel = if right_panel.is_some() {
                crate::theme::RIGHT_PANEL_W
            } else {
                0.0
            };
            Self::grid_metrics(width - breakpoint.sidebar().width() - panel)
        };

        Self {
            width,
            breakpoint,
            density: Density::Compact,
            sidebar_override: None,
            right_panel,
            lyrics: false,
            nav: NavSection::default(),
            nav_history: Vec::new(),
            nav_forward: Vec::new(),
            grid_columns,
            grid_card_width,
            _bounds: bounds,
        }
    }

    pub fn right_panel(&self) -> Option<PanelTab> {
        self.right_panel
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        let current = self.sidebar();
        let next = match current {
            SidebarState::Expanded => SidebarState::Icons,
            SidebarState::Icons => SidebarState::Hidden,
            SidebarState::Hidden => SidebarState::Expanded,
        };
        self.sidebar_override = Some(next);
        // 侧栏宽度变了，网格可用宽度跟着变，列数必须同步。
        self.recompute_grid_columns();
        cx.notify();
    }

    /// 切换右面板。传入当前已选中的页签表示「收起它」。
    pub fn toggle_panel(&mut self, tab: PanelTab, cx: &mut Context<Self>) {
        self.right_panel = if self.right_panel == Some(tab) {
            None
        } else {
            Some(tab)
        };
        self.recompute_grid_columns();
        cx.notify();
    }

    /// 收起当前打开的面板。浮层的关闭键与遮罩点击都走这里。
    pub fn close_panel(&mut self, cx: &mut Context<Self>) {
        if self.right_panel.take().is_some() {
            self.recompute_grid_columns();
            cx.notify();
        }
    }

    pub fn set_density(&mut self, density: Density, cx: &mut Context<Self>) {
        if self.density != density {
            self.density = density;
            cx.notify();
        }
    }

    /// 启动时应用落盘偏好，只在这一次生效：运行中回读它，手动收起就变成「关不掉」。
    /// 断点仍压过偏好，窄窗口不许硬撑展开。
    pub fn apply_settings(&mut self, settings: &crate::settings::Settings, cx: &mut Context<Self>) {
        self.set_density(settings.density, cx);
        self.set_nav(settings.default_view, cx);

        if self.breakpoint.right_panel_expanded() {
            self.right_panel = settings.default_panel;
        }
        if let Some(sidebar) = settings.default_sidebar {
            self.sidebar_override = Some(sidebar);
        }

        self.recompute_grid_columns();
        cx.notify();
    }

    pub fn toggle_density(&mut self, cx: &mut Context<Self>) {
        let next = self.density.toggle();
        self.set_density(next, cx);
    }
}
