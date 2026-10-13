//! 业务组件：认识 `Track` / `PlaybackEntity`，与 `common/` 相对。
//! 要不要 `observe` 播放实体只决定组件自己重不重算：祖先被标脏时它照样跟着重渲染。

pub mod album_grid;
pub mod bottom_bar;
pub mod detail;
pub mod home;
pub mod lyrics;
pub mod mini_now_playing;
pub mod now_playing;
pub mod queue_panel;
pub mod search_box;
pub mod search_page;
pub mod seek_bar;
pub mod settings_panel;
pub mod sidebar;
pub mod top_bar;
pub mod track_list;
pub mod transport;
pub mod volume;
pub mod window_controls;

pub use album_grid::{AlbumGrid, GridEvent};
pub use bottom_bar::BottomBar;
pub use home::HomeView;
pub use lyrics::LyricsView;
pub use mini_now_playing::{MiniNowPlaying, MiniPlayerEvent};
pub use now_playing::NowPlaying;
pub use queue_panel::QueuePanel;
pub use search_box::{SearchBox, SearchBoxEvent};
pub use search_page::SearchPage;
pub use seek_bar::SeekBar;
pub use settings_panel::{SettingsEntity, SettingsPanel};
pub use sidebar::Sidebar;
pub use top_bar::TopBar;
pub use track_list::{TrackList, TrackListEvent};
pub use transport::{Transport, TransportEvent};
pub use volume::VolumeControl;

use lyre_core::TrackId;

use crate::common::tooltip;
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon};
use crate::platform::*;
use crate::shell::NavSection;
use crate::theme;

/// 卡片封面右下角的悬浮播放键，只在 hover 时出现；`el_id` 由调用方保证唯一（同一首会同时出现在多个区块）。
/// 必须阻断冒泡，否则一次点击同时播放并触发卡片自身的点击。
pub(crate) fn card_play_button(
    el_id: impl Into<ElementId>,
    group: SharedString,
    track_id: TrackId,
    library: &Entity<LibraryEntity>,
    title: &str,
    kind: NavSection,
) -> AnyElement {
    // 点击闭包是 `'static`，队列用的名字得自己有一份，不能借用 `title`。
    let play_name = title.to_owned();
    div()
        .id(el_id)
        .absolute()
        .right(px(theme::GAP_SM))
        .bottom(px(theme::GAP_SM))
        .size(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme::control())
        .border(px(2.))
        .border_color(theme::bg())
        .opacity(0.0)
        .cursor_pointer()
        .group_hover(group, |style| style.opacity(1.0))
        // 键盘用户看不见 hover 态：给了 Tab 停靠点就得在聚焦时显形，否则 Enter 按下去屏幕没反应。
        .tab_index(crate::common::button::TAB_STOP)
        .focus(|style| style.opacity(1.0))
        .focus_visible(theme::focus_ring)
        .role(Role::Button)
        .hover(|style| {
            style
                .bg(theme::control_hover())
                .shadow(theme::hover_shadow())
        })
        .on_click({
            let library = library.clone();
            move |_, _, cx| {
                cx.stop_propagation();
                // 播放统一走 `activate_*`，不在这里自己改 cursor；队列按整张专辑 / 整位艺术家定。
                library.update(cx, |library, cx| match kind {
                    NavSection::Albums => library.activate_album(&play_name, track_id, cx),
                    _ => library.activate_artist(&play_name, track_id, cx),
                });
            }
        })
        .tooltip(tooltip::builder(format!("播放《{title}》")))
        .aria_label(format!("播放《{title}》"))
        .child(icon(Glyph::Play, 16., theme::text_on_control()))
        .into_any_element()
}
