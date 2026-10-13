//! GPUI 依赖的唯一入口：crates.io 上 `gpui` 有多个互不兼容、版本不互通的分叉，换分叉只改这个文件。
//! 约定：只有这里可以出现 `gpui` / `gpui_component` 这两个名字，其余文件一律 `use crate::platform::*;`。
//! `pub use gpui::*` 带入的同名 `test` 宏会遮蔽内置 `#[test]`，测试模块因此不 glob。

pub use gpui::*;

pub use gpui::prelude::*;

/// 组件库的窗口根。每个窗口的根视图都必须包一层，否则主题、浮层、焦点陷阱拿不到宿主上下文。
pub use gpui_component::Root;

/// 图标元素。gpui 把 SVG 渲染成纯 alpha 蒙版、只用 `text_color` 填色，SVG 里的 fill 无效。
pub use gpui_component::Icon;

/// 给 [`Icon`] 设置尺寸的 trait（`with_size`）。
pub use gpui_component::Sizable;

/// 文本输入。自带 `key_context("Input")`，根视图上的裸字母快捷键要靠 `&& !Input` 谓词避让，否则搜索框里打「s」会切换播放模式。
pub use gpui_component::input::{Input, InputEvent, InputState};

pub use gpui_component::ThemeMode;

/// 组件库的主题全局态，只管组件库控件内部的颜色；本项目的配色在 `crate::theme`。
pub use gpui_component::Theme;
/// 右键菜单。`context_menu` 只在右键打开；左键点击展开用 [`crate::common::menu::dropdown`]。
pub use gpui_component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};

/// 点击展开的浮层。触发器要 `Selectable`，本项目用 [`crate::common::menu::MenuTrigger`] 适配。
pub use gpui_component::popover::Popover;

/// 模态对话框。`window.open_dialog(cx, |dialog, ..| ..)` 由组件库的 `Root` 托管层与焦点，
/// 所以必须走 `WindowExt` 而不是自己画一层遮罩。
pub use gpui_component::WindowExt;

pub use gpui_component::Selectable;

/// 滚动条。gpui 自己不画滚动条，虚拟列表只维护偏移量，屏幕上什么都没有。
/// 组件库这个独立元素要的是 [`ScrollbarHandle`]，两个句柄都实现了它，拖动与点轨道不用自己接。
pub use gpui_component::scroll::{Scrollbar, ScrollbarHandle, ScrollbarMode};

pub fn application() -> Application {
    gpui_platform::application()
}

/// 初始化组件库。必须在开窗前调用，否则开窗时 panic `no state of type gpui_component::theme::Theme exists`。
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    // 组件库默认 Light，而 `Input` 的文字/光标/选区/占位四色取自组件库主题而不是本项目的 `theme.rs`：
    // 不切暗色就是黑字压深灰底，输入的内容看不见。界面只有暗色一套，所以也不跟随系统探测。
    Theme::change(ThemeMode::Dark, None, cx);

    Theme::update(cx, |theme| {
        theme.caret = crate::theme::control().into();
        theme.selection = crate::theme::control().opacity(0.35).into();
    });
}

pub fn rooted<T: Render + 'static>(
    view: Entity<T>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Root> {
    cx.new(|cx| Root::new(view, window, cx))
}
