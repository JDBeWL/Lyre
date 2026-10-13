//! 自绘窗口控件：最小化 / 最大化 / 还原 / 关闭。
//! Windows 上点击不走 GPUI 而是 `WM_NCHITTEST` → `DefWindowProc`，按钮因此自带系统级按下态；非 Windows 平台要 `on_click` 兜底。
//! 这些按钮必须是拖拽区的兄弟节点 —— 命中测试取第一个命中的条目。

use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::theme;

/// 顶栏圆按钮的统一直径。
const CTRL_PX: f32 = 32.0;
/// 窗口符号「— □ ×」的尺寸。
const GLYPH_PX: f32 = 12.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Control {
    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }

    fn glyph(self) -> Glyph {
        match self {
            Self::Minimize => Glyph::WindowMinimize,
            Self::Maximize => Glyph::WindowMaximize,
            Self::Restore => Glyph::WindowRestore,
            Self::Close => Glyph::WindowClose,
        }
    }

    fn tooltip(self) -> &'static str {
        self.glyph().label()
    }

    /// 用作 `ElementId` 第二段的序号。
    fn code(self) -> u32 {
        match self {
            Self::Minimize => 0,
            Self::Maximize => 1,
            Self::Restore => 2,
            Self::Close => 3,
        }
    }
}

/// 整个窗口控件组（最小化 / 最大化 / 关闭），最大化态取自 `window.is_maximized()`（GPUI 已缓存平台层状态，不必监听窗口事件）。
pub fn window_controls(window: &Window) -> impl IntoElement {
    // macOS 的红绿灯由系统绘制，这里返回空占位以保持顶栏结构一致。
    if cfg!(target_os = "macos") {
        return div().id("window-controls").flex_none();
    }

    // 没有 compositor 的 X11 会话下 minimize / maximize 都可能不被支持，硬画会给出点不动的按钮。
    let supported = window.window_controls();
    let maximized = window.is_maximized();

    let mut row = div()
        .id("window-controls")
        .flex_none()
        .flex()
        .items_center()
        .gap(px(theme::GAP_XS))
        .h_full();

    if supported.minimize {
        row = row.child(control_button(Control::Minimize));
    }
    if supported.maximize {
        row = row.child(control_button(if maximized {
            Control::Restore
        } else {
            Control::Maximize
        }));
    }
    // 关闭不受 `supported` 限制：关掉窗口必须始终可达。
    row.child(control_button(Control::Close))
}

fn control_button(control: Control) -> impl IntoElement {
    let is_close = control == Control::Close;
    // 每个按钮一个 group：符号是子元素，要靠 `group_hover` 才能感知整颗按钮的悬停。
    let group = SharedString::from(format!("winctl-{}", control.code()));

    div()
        .id(("window-ctl", control.code()))
        .group(group.clone())
        .size(px(CTRL_PX))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .when(cfg!(target_os = "windows"), |this| {
            this.window_control_area(control.area())
        })
        .hover(|style| {
            if is_close {
                style.bg(theme::danger())
            } else {
                style.bg(theme::elevated())
            }
        })
        .when(!cfg!(target_os = "windows"), |this| {
            this.cursor_pointer()
                .on_click(move |_, window, _| match control {
                    Control::Minimize => window.minimize_window(),
                    Control::Maximize | Control::Restore => window.zoom_window(),
                    Control::Close => window.remove_window(),
                })
        })
        .tooltip(crate::common::tooltip::builder(control.tooltip()))
        .child(window_glyph(control, group))
}

/// 窗口符号。颜色必须显式给常态与悬停两次：gpui 把 SVG 渲成 alpha 蒙版，颜色只由 `text_color` 决定，而子元素不继承父级的 `text_color`。
/// `icon_inherit` 返回的 `impl IntoElement` 已擦除链式方法，所以变色要靠这层 `div`。
fn window_glyph(control: Control, group: SharedString) -> impl IntoElement {
    // 关闭键悬停是红底：白字压 `#e5484d` 只有 3.4:1，达不到 AA，得用黑字。
    let idle = theme::text();
    let hovered = if control == Control::Close {
        theme::text_on_danger()
    } else {
        theme::text()
    };

    div()
        .flex()
        .items_center()
        .justify_center()
        .text_color(idle)
        .group_hover(group, |style| style.text_color(hovered))
        .child(icon_inherit(control.glyph(), GLYPH_PX))
}
