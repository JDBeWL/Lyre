//! 按钮：图标按钮 [`IconButton`]、文字按钮 [`text_button`]、胶囊 chip 与四种规格 [`ButtonKind`]。

use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::theme;

/// 图标按钮。链式配置，最后调 [`render`](IconButton::render)。
pub struct IconButton {
    id: ElementId,
    glyph: Glyph,
    box_size: f32,
    glyph_size: f32,
    color: Rgba,
    hover_color: Rgba,
    base_bg: Rgba,
    hover_bg: Rgba,
    radius: f32,
    enabled: bool,
    label: Option<&'static str>,
    on_click: Option<ClickHandler>,
}

/// 图标按钮的点击回调形状。到处写全 `Box<dyn Fn…>` 已经跨过 clippy 的复杂度上限。
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

impl IconButton {
    /// 默认：32px 方框、16px 图标、`text_muted` 起、悬停变 `text`、全圆角。
    pub fn new(id: impl Into<ElementId>, glyph: Glyph) -> Self {
        Self {
            id: id.into(),
            glyph,
            box_size: 32.0,
            glyph_size: 16.0,
            color: theme::text_muted(),
            hover_color: theme::text(),
            base_bg: theme::transparent(),
            hover_bg: theme::elevated(),
            radius: theme::RADIUS_PILL,
            enabled: true,
            label: None,
            on_click: None,
        }
    }

    pub fn box_size(mut self, size: f32) -> Self {
        self.box_size = size;
        self
    }

    pub fn glyph_size(mut self, size: f32) -> Self {
        self.glyph_size = size;
        self
    }

    /// 常态颜色。悬停色不在这里配，由 `new` 定成 `text`。
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = color;
        self
    }

    /// 禁用态：降级颜色且不绑事件。
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// 悬停提示。同时写进无障碍名称（`aria_label`）：图标按钮除了图标没有别的名字。
    pub fn label(mut self, label: &'static str) -> Self {
        self.label = Some(label);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    pub fn render(self) -> impl IntoElement {
        let IconButton {
            id,
            glyph,
            box_size,
            glyph_size,
            color,
            hover_color,
            base_bg,
            hover_bg,
            radius,
            enabled,
            label,
            on_click,
        } = self;

        let effective_color = if enabled { color } else { theme::text_faint() };

        let mut element = div()
            .id(id)
            .flex_none()
            .size(px(box_size))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius))
            .text_color(effective_color)
            .child(icon_inherit(glyph, glyph_size));

        if enabled {
            element = element
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg).text_color(hover_color))
                // 进 Tab 序列。gpui 没有默认的 Tab 绑定（`lyre-app::keymap` 自己绑），
                // 但聚焦后按 Enter / Space 触发 `on_click` 是内建的，不用自己接。
                .tab_index(TAB_STOP)
                .focus_visible(theme::focus_ring)
                .role(Role::Button);
            if let Some(handler) = on_click {
                element = element.on_click(handler);
            }
        } else {
            // 禁用态不给 Tab 停靠点：Tab 到一个按下去没反应的控件比没有它更糟。
            // 光标也要显式给默认箭头 —— 否则它沿用指针形状，看着仍像可点。
            element = element.cursor_default();
        }

        if base_bg != theme::transparent() {
            element = element.bg(base_bg);
        }

        if let Some(label) = label {
            element = element
                .aria_label(label)
                .tooltip(crate::common::tooltip::builder(label));
        }

        element
    }
}

/// Tab 序列里的位置。所有控件都用 0（按文档顺序走），只有真的需要插队时才另给值。
pub const TAB_STOP: isize = 0;

/// 按钮的几何规格：尺寸与字级只有这一个来源。
pub const BTN_PX: f32 = theme::GAP_LG;
pub const BTN_PY: f32 = theme::GAP_SM;
pub const BTN_FONT: f32 = theme::FONT_BODY;

/// 文字按钮的四种规格。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonKind {
    /// 实心强调色，一屏最多一个（主行动）
    Primary,
    /// 常驻灰底，次要但常驻的操作
    Secondary,
    /// 透明底，悬停才出现底色
    Ghost,
    /// 透明底 + 描边，下拉触发器用
    Outline,
}

impl ButtonKind {
    /// 纵向内边距。gpui 的盒子高度 = 内容 + padding + border，
    /// 所以带描边的规格减 1px，四种规格的外框高度才相同。
    pub fn pad_y(self) -> f32 {
        match self {
            Self::Outline => BTN_PY - 1.0,
            Self::Primary | Self::Secondary | Self::Ghost => BTN_PY,
        }
    }

    /// 外框高度（不含文字行高，只算 padding 与边框）。
    pub fn frame_height(self) -> f32 {
        let border = if self == Self::Outline { 2.0 } else { 0.0 };
        self.pad_y() * 2.0 + border
    }

    /// 常态底色 / 常态文字色 / 悬停底色 / 悬停文字色。
    fn colors(self) -> (Rgba, Rgba, Rgba, Rgba) {
        match self {
            Self::Primary => (
                theme::control(),
                theme::text_on_control(),
                theme::control_hover(),
                theme::text_on_control(),
            ),
            Self::Secondary => (
                theme::elevated(),
                theme::text(),
                theme::border(),
                theme::text(),
            ),
            // Ghost 与 Outline 的配色完全一致：两者的差别只在描边，那由
            // `pad_y` / `frame_height` 处理，与颜色无关。
            Self::Ghost | Self::Outline => (
                theme::transparent(),
                theme::text_muted(),
                theme::elevated(),
                theme::text(),
            ),
        }
    }
}

/// 按钮外壳：胶囊、内边距、字级、悬停反馈；内容由调用方 `.child(...)` 补，行为由 `.on_click(...)` 绑定。
pub fn button_shell(id: impl Into<ElementId>, kind: ButtonKind) -> Stateful<Div> {
    let (base_bg, base_color, hover_bg, hover_color) = kind.colors();

    let shell = div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .gap(px(theme::GAP_XS))
        .px(px(BTN_PX))
        .py(px(kind.pad_y()))
        // 圆角固定取胶囊值，不按尺寸派生
        .rounded(px(theme::RADIUS_PILL))
        .text_size(px(BTN_FONT))
        .font_weight(theme::WEIGHT_BUTTON)
        .text_color(base_color)
        .bg(base_bg)
        .cursor_pointer()
        // 与 `IconButton` 同一套键盘约定：进 Tab 序列、聚焦时画环、Enter / Space 由 gpui 转 click。
        .tab_index(TAB_STOP)
        .focus_visible(theme::focus_ring)
        .role(Role::Button)
        .hover(move |style| style.bg(hover_bg).text_color(hover_color));

    if kind == ButtonKind::Outline {
        shell.border_1().border_color(theme::border())
    } else {
        shell
    }
}

/// 胶囊选中态的两档配色：选中白底黑字、未选灰底灰字。
pub fn chip_colors(active: bool) -> (Rgba, Rgba) {
    if active {
        (theme::control(), theme::text_on_control())
    } else {
        (theme::elevated(), theme::text_muted())
    }
}

/// 胶囊「开关」按钮：图标 + 文字，选中态走 [`chip_colors`]。
pub fn toggle_chip(
    id: impl Into<ElementId>,
    glyph: Glyph,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    let (base_bg, base_color) = chip_colors(active);

    div()
        .id(id)
        // 可收缩而不是 `flex_none`：详情页头图那一排在 560px 窗口下放不下四件东西，
        // 不肯收缩就会被主区卡片裁掉最右的「更多」。文字由下面的 `truncate` 兜住。
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(theme::GAP_SM))
        .px(px(theme::GAP_MD))
        .py(px(6.))
        .rounded(px(theme::RADIUS_PILL))
        .tab_index(TAB_STOP)
        .focus_visible(theme::focus_ring)
        // 开关按钮：`active` 时除了配色还向读屏报「已按下」，不靠颜色单通道。
        .role(Role::Button)
        .aria_selected(active)
        .bg(base_bg)
        .text_color(base_color)
        .cursor_pointer()
        .when(!active, |chip| {
            chip.hover(|style| style.bg(theme::border()).text_color(theme::text()))
        })
        .text_size(px(theme::FONT_CAPTION))
        .font_weight(theme::WEIGHT_BUTTON)
        // 图标跟随父级 `text_color`（[`icon_inherit`] 的语义），不用再单独设色。
        .child(icon_inherit(glyph, 14.))
        .child(div().min_w_0().truncate().child(label))
}

pub fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    // `IntoElement` 只对具体类型实现，泛型 `impl Into<SharedString>` 要先落成 `SharedString`
    let label: SharedString = label.into();
    button_shell(id, kind).on_click(on_click).child(label)
}

#[cfg(test)]
mod tests {
    use super::ButtonKind;

    #[test]
    fn button_kinds_are_distinct() {
        assert_ne!(ButtonKind::Primary, ButtonKind::Ghost);
        assert_ne!(ButtonKind::Primary, ButtonKind::Secondary);
        assert_ne!(ButtonKind::Secondary, ButtonKind::Ghost);
        assert_ne!(ButtonKind::Outline, ButtonKind::Ghost);
    }

    #[test]
    fn every_kind_has_the_same_frame_height() {
        let kinds = [
            ButtonKind::Primary,
            ButtonKind::Secondary,
            ButtonKind::Ghost,
            ButtonKind::Outline,
        ];
        let expected = kinds[0].frame_height();
        for kind in kinds {
            assert_eq!(
                kind.frame_height(),
                expected,
                "{kind:?} 的外框高度和别的规格不一致 —— 并排时会一高一矮"
            );
        }
    }
}
