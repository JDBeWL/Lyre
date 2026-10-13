//! 矢量图标：描边类用 Lucide 路径（SVG 数据在 `icon_data.rs`，由脚本批量生成），
//! 填充类（运输控件、窗口控件）手写 —— Lucide 的 2px 描边缩到 10–16px 几乎看不见。

use crate::icon_data::stroke_svg;
use crate::platform::*;

/// 填充类图标的 SVG 外壳。字符串里不能出现 `#` —— `r#"..."#` 遇到 `"#` 会提前结束。
macro_rules! filled {
    ($($inner:literal),+ $(,)?) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor">"#,
            $($inner),+,
            "</svg>"
        )
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Glyph {
    // 填充类：运输控件
    Play,
    Pause,
    SkipBack,
    SkipForward,
    Stop,
    HeartFilled,

    // 描边类：见 icon_data.rs
    Search,
    Shuffle,
    RepeatAll,
    RepeatOne,
    List,
    Grid,
    Queue,
    Folder,
    Album,
    Artist,
    More,
    Heart,
    ChevronDown,
    ChevronRight,
    ArrowLeft,
    ArrowRight,
    Home,
    PanelLeft,
    /// 侧栏已折叠时用（箭头朝外），与 [`Glyph::PanelLeft`] 区分开合方向。
    PanelLeftOpen,
    VolumeHigh,
    VolumeLow,
    VolumeMuted,
    Check,
    Undo,
    Clock,
    Trash,
    Import,
    Settings,
    Cross,
    Plus,
    Sort,
    Music,
    /// 拖拽手柄（两列圆点）。手写成实心点：Lucide 的 `grip-vertical` 是描边圆，缩到 14px 会糊成一片。
    Grip,

    // 顶栏右上用户区
    Bell,
    User,

    // 底栏右侧的功能入口
    Lyrics,
    Devices,
    Fullscreen,
    PanelRight,

    // 窗口控件：手写的填充类，不在生成的那批里
    WindowMinimize,
    WindowMaximize,
    WindowRestore,
    WindowClose,
}

impl Glyph {
    /// 无障碍名称。用于 tooltip 与 `aria` 语义。
    pub fn label(self) -> &'static str {
        match self {
            Glyph::Play => "播放",
            Glyph::Pause => "暂停",
            Glyph::SkipBack => "上一曲",
            Glyph::SkipForward => "下一曲",
            Glyph::Stop => "停止",
            Glyph::HeartFilled => "取消收藏",
            Glyph::Search => "搜索",
            Glyph::Shuffle => "随机播放",
            Glyph::RepeatAll => "列表循环",
            Glyph::RepeatOne => "单曲循环",
            Glyph::List => "列表视图",
            Glyph::Grid => "网格视图",
            Glyph::Queue => "播放队列",
            Glyph::Folder => "文件夹",
            Glyph::Album => "专辑",
            Glyph::Artist => "艺术家",
            Glyph::More => "更多操作",
            Glyph::Heart => "收藏",
            Glyph::ChevronDown => "展开",
            Glyph::ChevronRight => "折叠",
            Glyph::ArrowLeft => "返回",
            Glyph::ArrowRight => "前进",
            Glyph::Home => "首页",
            Glyph::PanelLeft => "折叠侧栏",
            Glyph::PanelLeftOpen => "展开侧栏",
            Glyph::VolumeHigh => "音量",
            Glyph::VolumeLow => "音量较低",
            Glyph::VolumeMuted => "静音",
            Glyph::Check => "已选中",
            Glyph::Undo => "撤销",
            Glyph::Clock => "最近",
            Glyph::Trash => "移除",
            Glyph::Import => "导入",
            Glyph::Settings => "设置",
            Glyph::Cross => "关闭",
            Glyph::Plus => "添加",
            Glyph::Sort => "排序",
            Glyph::Music => "音乐",
            Glyph::Grip => "拖动排序",
            Glyph::Bell => "通知",
            Glyph::User => "账户",
            Glyph::Lyrics => "歌词",
            Glyph::Devices => "播放设备",
            Glyph::Fullscreen => "全屏",
            Glyph::PanelRight => "正在播放视图",
            Glyph::WindowMinimize => "最小化",
            Glyph::WindowMaximize => "最大化",
            Glyph::WindowRestore => "向下还原",
            Glyph::WindowClose => "关闭窗口",
        }
    }
}

/// gpui 把 SVG 渲染成纯 alpha 蒙版（只取 `p.alpha()`）再用 `text_color` 填色，
/// 所以 SVG 里写的 `fill` / `stroke` 一律无效，`currentColor` 也不会被解析。
pub fn icon(glyph: Glyph, size: f32, color: Rgba) -> impl IntoElement {
    Icon::default()
        .data(svg_source(glyph).as_bytes())
        .with_size(px(size))
        .text_color(color)
}

/// 颜色继承父级的图标：不设色时 `Icon` 回退到 `window.text_style().color`，
/// 因此能跟随 `.hover(|s| s.text_color(..))`；[`icon`] 设了色就烘死在子元素上。
pub fn icon_inherit(glyph: Glyph, size: f32) -> impl IntoElement {
    Icon::default()
        .data(svg_source(glyph).as_bytes())
        .with_size(px(size))
}

/// 取图标的 SVG 源码。填充类返回手写字面量，其余走生成的数据。
fn svg_source(glyph: Glyph) -> &'static str {
    match glyph {
        Glyph::Play => filled!(
            r#"<path d="M8 6.2v11.6c0 1.1 1.2 1.78 2.14 1.22l9.9-5.8a1.42 1.42 0 0 0 0-2.44l-9.9-5.8A1.42 1.42 0 0 0 8 6.2Z"/>"#
        ),
        Glyph::Pause => filled!(
            r#"<rect x="6.5" y="4" width="4" height="16" rx="1.4"/>"#,
            r#"<rect x="13.5" y="4" width="4" height="16" rx="1.4"/>"#
        ),
        Glyph::SkipBack => filled!(
            r#"<rect x="4.5" y="4.5" width="2.8" height="15" rx="1.2"/>"#,
            r#"<path d="M19.5 6.2v11.6c0 1.1-1.2 1.78-2.14 1.22l-9.9-5.8a1.42 1.42 0 0 1 0-2.44l9.9-5.8A1.42 1.42 0 0 1 19.5 6.2Z"/>"#
        ),
        Glyph::SkipForward => filled!(
            r#"<rect x="16.7" y="4.5" width="2.8" height="15" rx="1.2"/>"#,
            r#"<path d="M4.5 6.2v11.6c0 1.1 1.2 1.78 2.14 1.22l9.9-5.8a1.42 1.42 0 0 0 0-2.44l-9.9-5.8A1.42 1.42 0 0 0 4.5 6.2Z"/>"#
        ),
        Glyph::Stop => filled!(r#"<rect x="6" y="6" width="12" height="12" rx="2"/>"#),
        Glyph::Grip => filled!(
            r#"<circle cx="9" cy="6" r="1.6"/>"#,
            r#"<circle cx="9" cy="12" r="1.6"/>"#,
            r#"<circle cx="9" cy="18" r="1.6"/>"#,
            r#"<circle cx="15" cy="6" r="1.6"/>"#,
            r#"<circle cx="15" cy="12" r="1.6"/>"#,
            r#"<circle cx="15" cy="18" r="1.6"/>"#
        ),
        Glyph::HeartFilled => filled!(
            r#"<path d="M12 21.1l-1.38-1.26C5.5 15.24 2.2 12.24 2.2 8.55 2.2 5.55 4.55 3.2 7.55 3.2c1.7 0 3.33.79 4.45 2.04 1.12-1.25 2.75-2.04 4.45-2.04 3 0 5.35 2.35 5.35 5.35 0 3.69-3.3 6.69-8.42 11.29L12 21.1z"/>"#
        ),
        // 窗口控件不走 Lucide：2px 描边缩到 10px 只剩 0.83px，光栅化后不保证可见。
        Glyph::WindowMinimize => filled!(r#"<rect x="5" y="11.5" width="14" height="1"/>"#),
        Glyph::WindowMaximize => filled!(
            r#"<rect x="5.5" y="5.5" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1"/>"#
        ),
        Glyph::WindowRestore => filled!(
            r#"<rect x="8.5" y="3.5" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1" opacity="0.55"/>"#,
            r#"<rect x="3.5" y="8.5" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1"/>"#
        ),
        Glyph::WindowClose => filled!(
            r#"<path d="M5.5 5.5 18.5 18.5M18.5 5.5 5.5 18.5" fill="none" stroke="currentColor" stroke-width="1"/>"#
        ),
        other => stroke_svg(other),
    }
}
