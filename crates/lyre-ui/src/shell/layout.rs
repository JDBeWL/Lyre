//! 窗口几何：断点档位、侧栏宽度、行密度、专辑网格的列数与卡宽。
//! 都是「给定宽度算出布局参数」的纯计算，入口是 [`ShellEntity::apply_viewport`]。

use crate::platform::*;

use super::{PanelTab, ShellEntity};

/// 布局断点，宽 / 中 / 窄三档。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Breakpoint {
    /// ≥ 1180px：三栏齐全
    Wide,
    /// 860–1179px：右面板收起为图标条
    Medium,
    /// < 860px：侧栏收起为图标条、列表隐藏专辑列
    Narrow,
}

/// 网格卡片的宽度区间，列数由它反推：低于下限放不下可辨识的封面 + 标题，高于上限卡片成横幅。
pub const CARD_MIN_W: f32 = 132.0;
pub const CARD_MAX_W: f32 = 200.0;
/// 列数硬上限，避免超宽屏（4K+）出现十几列。
pub const MAX_GRID_COLUMNS: usize = 8;
/// 网格四周外边距，必须与 `album_grid` 的 `PAD` 一致。
const GRID_PAD: f32 = crate::theme::GAP_LG;
/// 卡片间距，必须与 `album_grid` 的 `GAP` 一致。
const GRID_GAP: f32 = crate::theme::GAP_MD;

pub const WIDE_MIN: f32 = 1180.0;
pub const MEDIUM_MIN: f32 = 860.0;
/// 底栏改为两行（传输区独占一行）的阈值。
pub const STACKED_BOTTOM_MIN: f32 = 640.0;
/// 必须低于 `MEDIUM_MIN`：否则「是否堆叠底栏」按档位和按宽度会算出两种结果。
const _: () = assert!(STACKED_BOTTOM_MIN < MEDIUM_MIN);

impl Breakpoint {
    pub fn for_width(width: f32) -> Self {
        if width >= WIDE_MIN {
            Self::Wide
        } else if width >= MEDIUM_MIN {
            Self::Medium
        } else {
            Self::Narrow
        }
    }

    pub fn sidebar(self) -> SidebarState {
        match self {
            Self::Wide | Self::Medium => SidebarState::Expanded,
            Self::Narrow => SidebarState::Icons,
        }
    }

    /// 右侧面板默认是否展开。窄窗口下改为点击后以浮层出现。
    pub fn right_panel_expanded(self) -> bool {
        matches!(self, Self::Wide)
    }

    /// 列表是否显示「专辑」列。
    pub fn shows_album_column(self) -> bool {
        !matches!(self, Self::Narrow)
    }

    pub fn stacks_bottom_bar(self) -> bool {
        // 按档位只是近似，真实阈值看实际宽度（见 `ShellEntity::stacks_bottom_bar`）。
        matches!(self, Self::Narrow)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SidebarState {
    /// 图标 + 文字
    Expanded,
    /// 只有图标
    Icons,
    /// 完全隐藏
    Hidden,
}

impl SidebarState {
    pub fn width(self) -> f32 {
        match self {
            Self::Expanded => crate::theme::SIDEBAR_W,
            Self::Icons => crate::theme::SIDEBAR_ICON_W,
            Self::Hidden => 0.0,
        }
    }

    pub fn shows_labels(self) -> bool {
        matches!(self, Self::Expanded)
    }

    /// 展开程度：`Expanded`(2) > `Icons`(1) > `Hidden`(0)。生效形态取手动值与断点默认里数值小的那个。
    fn openness(self) -> u8 {
        match self {
            Self::Expanded => 2,
            Self::Icons => 1,
            Self::Hidden => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Density {
    /// 40px 行高，默认
    Compact,
    /// 52px 行高
    Comfortable,
}

impl Density {
    pub fn row_height(self) -> f32 {
        match self {
            Self::Compact => crate::theme::ROW_H_COMPACT,
            Self::Comfortable => crate::theme::ROW_H_COMFORTABLE,
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Compact => Self::Comfortable,
            Self::Comfortable => Self::Compact,
        }
    }
}

impl ShellEntity {
    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn breakpoint(&self) -> Breakpoint {
        self.breakpoint
    }

    /// 生效形态 = 手动值（`sidebar_override`）与断点默认里更收起的那个：手动收起后拉宽不弹开，
    /// 窄窗口也不硬撑展开。`toggle_sidebar` 从这里起循环。
    pub fn sidebar(&self) -> SidebarState {
        let by_breakpoint = self.breakpoint.sidebar();
        match self.sidebar_override {
            Some(over) if over.openness() < by_breakpoint.openness() => over,
            _ => by_breakpoint,
        }
    }

    /// 中间卡片的宽度（不含自身内边距），须与 `root/render.rs::main_content` 的排布一致。
    pub fn center_width(&self) -> f32 {
        Self::center_width_for(self.width, self.sidebar(), self.right_panel.is_some())
    }

    /// [`Self::center_width`] 的纯函数版，不依赖实体状态。
    pub fn center_width_for(width: f32, sidebar: SidebarState, has_right_panel: bool) -> f32 {
        let gutter = crate::theme::GUTTER;
        // 主区左右外边距；每多一个兄弟卡片，中间就多一条缝。
        let mut taken = gutter * 2.0;
        if sidebar != SidebarState::Hidden {
            taken += sidebar.width() + gutter;
        }
        if has_right_panel {
            taken += crate::theme::RIGHT_PANEL_W + gutter;
        }
        (width - taken).max(0.0)
    }

    pub fn density(&self) -> Density {
        self.density
    }

    /// 专辑网格当前每行几列。`album_grid` 直接读它。
    pub fn grid_columns(&self) -> usize {
        self.grid_columns.max(1)
    }

    /// 专辑网格单张卡片的预测宽度，`album_grid` 用它决定封面高度：`uniform_list` 要求行等高，封面不能自适应。
    /// 返回 0.0 表示宽度还不可用（窗口极窄），调用方应退回到 `COVER_H_MIN`。
    pub fn grid_card_width(&self) -> f32 {
        self.grid_card_width
    }

    pub fn row_height(&self) -> f32 {
        self.density.row_height()
    }

    /// 底栏是否改成两行：按实际宽度而非档位，640px 的阈值落在 `Narrow` 档内部。
    pub fn stacks_bottom_bar(&self) -> bool {
        self.width < STACKED_BOTTOM_MIN
    }

    /// 专辑网格每行几列：按该宽度下的默认布局（三栏 / 两栏 / 侧栏收成图标条）推导，供测试与首次开窗使用。
    /// 运行期真实列数走 `ShellEntity::recompute_grid_columns`，它按实际的侧栏形态和右面板开合来算。
    pub fn grid_columns_for(width: f32) -> usize {
        Self::grid_metrics_for(width).0
    }

    /// 公开版的 `grid_metrics`：按宽度的默认布局返回 `(列数, 单卡宽度)`。
    pub fn grid_metrics_for(width: f32) -> (usize, f32) {
        let breakpoint = Breakpoint::for_width(width);
        let panel = if breakpoint.right_panel_expanded() {
            crate::theme::RIGHT_PANEL_W
        } else {
            0.0
        };
        Self::grid_metrics(width - breakpoint.sidebar().width() - panel)
    }

    /// 按可用宽度和卡片宽度区间反推列数；取整只用下限，上限由 `album_grid` 的 `max_w` 裁。
    #[cfg(test)]
    fn columns_in(available: f32) -> usize {
        Self::grid_metrics(available).0
    }

    /// 由可用宽度算出 `(列数, 单卡宽度)`，两者必须同源算。
    /// 卡宽是预测值：一行 N 个等宽 `flex_1` 子项加 N-1 个固定间距，taffy 算出来就是这个值。
    pub(super) fn grid_metrics(available: f32) -> (usize, f32) {
        let usable = (available - GRID_PAD * 2.0).max(0.0);
        if usable <= 0.0 {
            return (1, 0.0);
        }

        // 分母含 GAP、实际只占 (n-1) 个 GAP：最后一张卡后面没有间距。
        let columns = ((usable + GRID_GAP) / (CARD_MIN_W + GRID_GAP))
            .floor()
            .max(1.0) as usize;
        let columns = columns.min(MAX_GRID_COLUMNS);

        let gaps = GRID_GAP * (columns.saturating_sub(1)) as f32;
        ((columns), ((usable - gaps) / columns as f32).max(0.0))
    }

    /// 底栏左侧的迷你曲目信息是否显示，只管内容不管几何（宽度见 [`Self::bottom_left_width`]）。
    pub fn shows_mini_now_playing(&self) -> bool {
        self.width >= MEDIUM_MIN
    }

    /// 底栏左栏宽度：随窗口连续收缩而不是两态，否则中间列会整组横向瞬移。
    /// 两端是 [`STACKED_BOTTOM_MIN`] 处 16px 占位、[`MEDIUM_MIN`] 处满宽 [`crate::theme::BOTTOM_LEFT_W`]。
    pub fn bottom_left_width(&self) -> f32 {
        let span = MEDIUM_MIN - STACKED_BOTTOM_MIN;
        let t = ((self.width - STACKED_BOTTOM_MIN) / span).clamp(0.0, 1.0);
        crate::theme::GAP_LG + (crate::theme::BOTTOM_LEFT_W - crate::theme::GAP_LG) * t
    }

    /// 重算列数与单卡宽度。返回 `true` 时调用方必须 notify：数据按列数切行、封面高度按卡宽算。
    pub(super) fn recompute_grid_columns(&mut self) -> bool {
        let panel = if self.right_panel.is_some() {
            crate::theme::RIGHT_PANEL_W
        } else {
            0.0
        };
        let (columns, card_width) = Self::grid_metrics(self.width - self.sidebar().width() - panel);

        // 卡宽按 0.5px 量化再比较：浮点尾数抖动会伪装成「宽度变了」，每 200ms 重绘一次整个网格。
        let changed =
            columns != self.grid_columns || (card_width - self.grid_card_width).abs() >= 0.5;

        self.grid_columns = columns;
        self.grid_card_width = card_width;
        changed
    }

    /// 重算宽度 / 断点 / 网格列数，返回是否需要重绘（跨断点或列数变化）。
    /// 不碰 `sidebar_override`：改写它会把手动收起的侧栏弹开。
    fn resize_to(&mut self, width: f32) -> bool {
        if (width - self.width).abs() < f32::EPSILON {
            return false;
        }
        let previous_breakpoint = self.breakpoint;
        self.width = width;
        self.breakpoint = Breakpoint::for_width(width);

        let crossed = self.breakpoint != previous_breakpoint;
        if crossed
            // 右面板只有开 / 关两态、没有手动值，窄档收起它是布局约束而不是改掉用户偏好。
            && !self.breakpoint.right_panel_expanded()
            && matches!(self.right_panel, Some(PanelTab::NowPlaying))
        {
            self.right_panel = None;
        }

        // 小幅缩放不重绘：flex 自适应，元素树不必重建。
        // 列数要单独判 —— 数据按列数切行交给 `uniform_list`，列数变了就得重建行。
        crossed || self.recompute_grid_columns()
    }

    /// `new()` 注册的窗口尺寸订阅：`resize_to` 返回 true 才 `notify`。
    pub(super) fn apply_viewport(&mut self, width: f32, cx: &mut Context<Self>) {
        if self.resize_to(width) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Breakpoint, CARD_MIN_W, Density, GRID_GAP, GRID_PAD, MAX_GRID_COLUMNS, MEDIUM_MIN,
        STACKED_BOTTOM_MIN, ShellEntity, SidebarState, WIDE_MIN,
    };
    use crate::platform::Subscription;
    use crate::shell::NavSection;
    use crate::theme;

    /// 只设宽度与手动侧栏形态的实体，其余字段取默认值。
    fn shell_at(width: f32, sidebar_override: Option<SidebarState>) -> ShellEntity {
        ShellEntity {
            width,
            breakpoint: Breakpoint::for_width(width),
            density: Density::Compact,
            sidebar_override,
            right_panel: None,
            lyrics: false,
            nav: NavSection::default(),
            nav_history: Vec::new(),
            nav_forward: Vec::new(),
            grid_columns: ShellEntity::grid_columns_for(width),
            grid_card_width: 0.0,
            _bounds: Subscription::new(|| {}),
        }
    }

    /// 一次窗口 resize，走生产路径 `resize_to`（即 `apply_viewport` 的计算部分）。
    fn resize(shell: &mut ShellEntity, width: f32) {
        shell.resize_to(width);
    }

    /// 独立反解单卡宽度的另一份算式，不与 `grid_metrics` 共享中间量。
    fn card_width_for(width: f32) -> f32 {
        let breakpoint = Breakpoint::for_width(width);
        let panel = if breakpoint.right_panel_expanded() {
            theme::RIGHT_PANEL_W
        } else {
            0.0
        };
        let columns = ShellEntity::grid_columns_for(width);
        let usable = width - breakpoint.sidebar().width() - panel - GRID_PAD * 2.0;
        (usable - GRID_GAP * (columns.saturating_sub(1)) as f32) / columns as f32
    }

    #[test]
    fn breakpoints_map_to_the_documented_layouts() {
        assert_eq!(Breakpoint::for_width(1920.0), Breakpoint::Wide);
        assert_eq!(Breakpoint::for_width(WIDE_MIN), Breakpoint::Wide);
        assert_eq!(Breakpoint::for_width(WIDE_MIN - 1.0), Breakpoint::Medium);
        assert_eq!(Breakpoint::for_width(MEDIUM_MIN), Breakpoint::Medium);
        assert_eq!(Breakpoint::for_width(MEDIUM_MIN - 1.0), Breakpoint::Narrow);
    }

    #[test]
    fn wide_keeps_every_region() {
        let wide = Breakpoint::for_width(1600.0);
        assert_eq!(wide.sidebar(), SidebarState::Expanded);
        assert!(wide.right_panel_expanded());
        assert!(wide.shows_album_column());
    }

    #[test]
    fn a_manually_collapsed_sidebar_stays_collapsed_when_the_window_grows() {
        let mut shell = shell_at(1600.0, Some(SidebarState::Icons));
        assert_eq!(shell.sidebar(), SidebarState::Icons, "手动收起后应是图标态");

        resize(&mut shell, 800.0);
        assert_eq!(shell.sidebar(), SidebarState::Icons, "窄档下仍是收起的样子");
        resize(&mut shell, 1600.0);
        assert_eq!(
            shell.sidebar(),
            SidebarState::Icons,
            "拉宽回来不该自己弹开 —— 用户没再动过它"
        );
    }

    #[test]
    fn resizing_never_touches_the_manual_sidebar_choice() {
        let mut shell = shell_at(1600.0, Some(SidebarState::Icons));
        for width in [800.0, 1000.0, 1400.0, 1920.0, 700.0, 1000.0, 1600.0] {
            resize(&mut shell, width);
            assert_eq!(
                shell.sidebar_override,
                Some(SidebarState::Icons),
                "resize 到 {width}px 时把手动值改掉了 —— 侧栏会自己弹开"
            );
        }
        assert_eq!(shell.sidebar(), SidebarState::Icons, "到头来仍是收起的样子");
    }

    #[test]
    fn a_manually_expanded_sidebar_still_collapses_in_a_narrow_window() {
        let mut shell = shell_at(1600.0, Some(SidebarState::Expanded));
        assert_eq!(shell.sidebar(), SidebarState::Expanded);

        resize(&mut shell, 800.0);
        assert_eq!(
            shell.sidebar(),
            SidebarState::Icons,
            "窄档的断点默认压过手动展开值"
        );
    }

    #[test]
    fn a_hidden_sidebar_stays_hidden_at_every_width() {
        for width in [800.0, 1000.0, 1400.0, 1920.0] {
            let mut shell = shell_at(width, Some(SidebarState::Hidden));
            for step in [1920.0, 800.0, 1600.0, 1000.0] {
                resize(&mut shell, step);
                assert_eq!(
                    shell.sidebar(),
                    SidebarState::Hidden,
                    "{width}px 起、拖到 {step}px 之后被撬开了"
                );
            }
        }
    }

    #[test]
    fn toggling_in_a_narrow_window_still_cycles() {
        let narrow = shell_at(800.0, None);
        assert_eq!(narrow.sidebar(), SidebarState::Icons, "窄档默认是图标态");

        // 窄档点了「展开」：手动值更展开，被断点默认压回 `Icons`
        let after = shell_at(800.0, Some(SidebarState::Expanded));
        assert_eq!(after.sidebar(), SidebarState::Icons);
    }

    #[test]
    fn narrow_collapses_sidebar_and_drops_album_column() {
        let narrow = Breakpoint::for_width(800.0);
        assert_eq!(narrow.sidebar(), SidebarState::Icons);
        assert!(!narrow.right_panel_expanded());
        assert!(
            !narrow.shows_album_column(),
            "窄窗口下专辑列的宽度应该让给标题"
        );
    }

    #[test]
    fn sidebar_states_report_their_widths() {
        assert_eq!(SidebarState::Expanded.width(), theme::SIDEBAR_W);
        assert_eq!(SidebarState::Icons.width(), theme::SIDEBAR_ICON_W);
        assert_eq!(SidebarState::Hidden.width(), 0.0);
        assert!(
            SidebarState::Expanded.width() > SidebarState::Icons.width(),
            "展开态应当宽于图标态"
        );
        assert!(SidebarState::Expanded.shows_labels());
        assert!(!SidebarState::Icons.shows_labels());
    }

    #[test]
    fn density_toggles_between_the_two_documented_row_heights() {
        assert_eq!(Density::Compact.row_height(), 40.0);
        assert_eq!(Density::Comfortable.row_height(), 52.0);
        assert_eq!(Density::Compact.toggle(), Density::Comfortable);
        assert_eq!(Density::Comfortable.toggle(), Density::Compact);
    }

    #[test]
    fn grid_columns_stay_in_range() {
        for width in [560.0, 860.0, 1180.0, 1920.0, 3840.0] {
            let columns = ShellEntity::grid_columns_for(width);
            assert!(
                (1..=MAX_GRID_COLUMNS).contains(&columns),
                "宽 {width}px 得到 {columns} 列，越界"
            );
        }
    }

    #[test]
    fn grid_cards_stay_legible_at_every_supported_width() {
        // 560px 是 `main.rs` 里设的 `window_min_size`。
        for width in [560.0, 640.0, MEDIUM_MIN, 900.0, WIDE_MIN] {
            let card_w = card_width_for(width);
            assert!(
                card_w >= CARD_MIN_W - 0.5,
                "宽 {width}px 时卡片宽 {card_w:.1}px，低于下限 {CARD_MIN_W}"
            );
        }
    }

    #[test]
    fn grid_cards_widen_with_the_window_but_stay_bounded() {
        let narrow = card_width_for(WIDE_MIN);
        let wide = card_width_for(2560.0);
        assert!(
            wide > narrow,
            "窗口从 {WIDE_MIN}px 拉宽到 2560px，卡片宽度应从 {narrow:.1} 增长到 {wide:.1}"
        );
    }

    #[test]
    fn grid_columns_grow_monotonically_within_a_breakpoint() {
        // 同一断点内窗口越宽列数只能不变或增加，否则会「拖宽反而少一列」。
        for (low, high) in [
            (560.0, MEDIUM_MIN),
            (MEDIUM_MIN, WIDE_MIN),
            (WIDE_MIN, 3840.0),
        ] {
            let mut previous = ShellEntity::grid_columns_for(low);
            let mut step = 20.0;
            while low + step < high {
                let width = low + step;
                let columns = ShellEntity::grid_columns_for(width);
                assert!(
                    columns >= previous,
                    "{low}–{high}px 区间内，宽度 {width}px 时列数从 {previous} 掉到 {columns}"
                );
                previous = columns;
                step += 20.0;
            }
        }
    }

    #[test]
    fn crossing_into_wide_costs_the_grid_a_column() {
        // 已知非单调点：跨过 WIDE_MIN 时右面板自动展开、主区少 320px，列数下降。
        // 这不是 bug，卡宽仍须在下限之上，由下一个断言兜住。
        let before = ShellEntity::grid_columns_for(WIDE_MIN - 1.0);
        let after = ShellEntity::grid_columns_for(WIDE_MIN);
        assert!(
            after < before,
            "右面板展开理应让列数从 {before} 降到 {after}；\
             若此断言失败说明右面板已不再占用网格宽度，下面的宽度断言需重估"
        );
        assert!(
            card_width_for(WIDE_MIN) >= CARD_MIN_W - 0.5,
            "少一列之后卡片仍不该被挤到 {}px 以下，实际 {:.1}px",
            CARD_MIN_W,
            card_width_for(WIDE_MIN)
        );
    }

    #[test]
    fn center_width_matches_the_main_area_it_describes() {
        // 1180px 是 Wide 下限：主区左右各 8、侧栏 232、右面板 320、中间两条缝各 8，剩下 596。
        assert_eq!(
            ShellEntity::center_width_for(1180.0, SidebarState::Expanded, true),
            596.0
        );
        assert_eq!(
            ShellEntity::center_width_for(1000.0, SidebarState::Hidden, false),
            1000.0 - theme::GUTTER * 2.0
        );
    }

    /// 底栏左栏宽度必须连续，不许在 `MEDIUM_MIN` 处跳变：中间列靠 `justify_center` 居中，跳变会让它整组瞬移。
    #[test]
    fn bottom_left_width_never_jumps_across_the_medium_threshold() {
        let mut previous = shell_at(STACKED_BOTTOM_MIN, None).bottom_left_width();
        let mut width = STACKED_BOTTOM_MIN + 8.0;
        while width <= MEDIUM_MIN + 40.0 {
            let current = shell_at(width, None).bottom_left_width();
            let delta = current - previous;
            assert!(
                delta >= -f32::EPSILON,
                "左栏宽度在 {width}px 处反而缩小了 {delta:.1}px —— 应当单调不减"
            );
            assert!(
                delta <= 12.0,
                "左栏宽度在 {width}px 处跳了 {delta:.1}px —— 中间列会跟着瞬移一半的距离"
            );
            previous = current;
            width += 8.0;
        }
    }

    #[test]
    fn bottom_left_width_spans_the_documented_range_and_clamps() {
        let bottom = shell_at(STACKED_BOTTOM_MIN, None).bottom_left_width();
        assert!(
            (bottom - theme::GAP_LG).abs() < 0.01,
            "堆叠阈值处应当只剩一个 {:.0}px 占位，实际 {bottom:.1}px",
            theme::GAP_LG
        );
        let full = shell_at(MEDIUM_MIN, None).bottom_left_width();
        assert!(
            (full - theme::BOTTOM_LEFT_W).abs() < 0.01,
            "中档处应当是满宽 {}px，实际 {full:.1}px",
            theme::BOTTOM_LEFT_W
        );
        // 区间之外继续夹紧：更窄不缩成负数，更宽不无限长。
        assert!((shell_at(400.0, None).bottom_left_width() - bottom).abs() < 0.01);
        assert!((shell_at(3840.0, None).bottom_left_width() - full).abs() < 0.01);
    }

    #[test]
    fn hiding_the_sidebar_adds_room_for_another_column() {
        // 隐藏侧栏腾出的宽度必须真的换来多一列
        let width = WIDE_MIN;
        let with_sidebar = ShellEntity::columns_in(width - theme::SIDEBAR_W);
        let without = ShellEntity::columns_in(width);
        assert!(
            without > with_sidebar,
            "隐藏侧栏后可用宽度多了 {}px，列数应从 {with_sidebar} 增到 {without}",
            theme::SIDEBAR_W
        );
    }

    #[test]
    fn grid_column_changes_are_coarse_enough_to_notify_sparingly() {
        // 1px 抖动就改列数会作废「断点内不 notify」的约定
        let base = ShellEntity::grid_columns_for(1600.0);
        for delta in [-1.0, 0.0, 1.0] {
            assert_eq!(
                ShellEntity::grid_columns_for(1600.0 + delta),
                base,
                "1px 级抖动不该改变列数"
            );
        }
    }
}
