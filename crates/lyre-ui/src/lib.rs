//! `lyre-ui` —— GPUI 视图层，实体之间用 `observe`（整体变化）与 `subscribe`（语义事件）连接。
//! 高频进度只重绘订阅了 `PlaybackEntity` 的组件，曲目列表与右侧面板不受影响。

pub mod actions;
/// 歌单落盘（`collections.json`）读写。
pub mod collections_store;
pub mod common;
pub mod components;
pub mod cover;
pub mod entities;
/// 「关注」艺术家集合与持久化。
pub mod follows;
pub mod icon;
pub mod icon_data;
/// 曲库缓存（`library.json`）读写。
pub mod library_store;
/// 「喜欢」集合与持久化。
pub mod likes;
pub mod logging;
/// 统一的 JSON 持久化层：数据目录定位、原子替换、七份 store 共用的读写语义。
pub mod persist;
pub mod platform;
pub mod recents;
/// 上次播放位置的读写（`resume.json`）。
pub mod resume;
pub mod root;
pub mod settings;
pub mod shell;
/// 键位与步进量的唯一来源，`lyre-app` 的绑定和设置页的展示都读它。
pub mod shortcuts;
pub mod theme;

pub use actions::*;
pub use cover::CoverCache;
pub use entities::{LibraryEntity, LibraryEvent, PlaybackEntity, PlaybackEvent, Step};
pub use root::RootView;
pub use shell::{Breakpoint, Density, NavSection, PanelTab, ShellEntity, SidebarState};
