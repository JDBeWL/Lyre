//! 底部控制栏：三列网格 —— 左曲目身份 / 中运输 + 进度 / 右模式 · 音量 · 队列。
//! 左右两栏都是显式宽度（左 16→240 随窗口连续变化，右固定 320），窄于 `STACKED_BOTTOM_MIN`（640px）时改两行、进度条独占一行。

use crate::components::mini_now_playing::MiniNowPlaying;
use crate::components::seek_bar::SeekBar;
use crate::components::settings_panel::SettingsEntity;
use crate::components::transport::Transport;
use crate::components::volume::VolumeControl;
use crate::entities::{LibraryEntity, PlaybackEntity, PlaybackEvent};
use crate::icon::Glyph;
use crate::platform::*;
use crate::shell::{PanelTab, ShellEntity};
use crate::theme;

pub struct BottomBar {
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
    playback: Entity<PlaybackEntity>,
    settings: Entity<SettingsEntity>,
    mini: Entity<MiniNowPlaying>,
    transport: Entity<Transport>,
    seek: Entity<SeekBar>,
    volume: Entity<VolumeControl>,
    _subs: Vec<Subscription>,
}

impl BottomBar {
    pub fn new(
        shell: Entity<ShellEntity>,
        library: Entity<LibraryEntity>,
        playback: Entity<PlaybackEntity>,
        settings: Entity<SettingsEntity>,
        mini: Entity<MiniNowPlaying>,
        transport: Entity<Transport>,
        seek: Entity<SeekBar>,
        volume: Entity<VolumeControl>,
        cx: &mut Context<Self>,
    ) -> Self {
        // 模式按钮的图标要跟着模式变，所以得 observe 曲库（低频，可接受）
        let library_sub = cx.observe(&library, |_, _, cx| cx.notify());
        let shell_sub = cx.observe(&shell, |_, _, cx| cx.notify());
        // 设备列表只在插拔或切换时变；subscribe 而不是 observe，免得被 10Hz 的进度通知带着重绘
        let device_sub = cx.subscribe(&playback, |_, _, event: &PlaybackEvent, cx| {
            if matches!(event, PlaybackEvent::DevicesChanged) {
                cx.notify();
            }
        });

        Self {
            shell,
            library,
            playback,
            settings,
            mini,
            transport,
            seek,
            volume,
            _subs: vec![library_sub, shell_sub, device_sub],
        }
    }
}

/// 选中项前面加勾。
fn mark_if(selected: bool, text: &str) -> String {
    if selected {
        format!("✓ {text}")
    } else {
        text.to_owned()
    }
}

/// 设备菜单。列表在展开这一刻才读：插拔随时会改它，预造一份只会显示过期数据。
fn device_menu(
    mut menu: PopupMenu,
    playback: &Entity<PlaybackEntity>,
    settings: &Entity<SettingsEntity>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let devices = playback.read(cx).device_list().to_vec();
    let current = playback.read(cx).current_device().map(str::to_owned);

    {
        let playback = playback.clone();
        let settings = settings.clone();
        menu = menu.item(
            PopupMenuItem::new(mark_if(current.is_none(), "跟随系统默认")).on_click(
                move |_, _, cx| {
                    playback.update(cx, |playback, cx| playback.set_device(None, cx));
                    settings.update(cx, |settings, cx| settings.set_preferred_device(None, cx));
                },
            ),
        );
    }

    if !devices.is_empty() {
        menu = menu.item(PopupMenuItem::separator());
    }

    for device in devices {
        // 跟随默认时那一台就是当前生效的设备；显式选定过则只勾选中的那台
        let selected = match &current {
            Some(id) => *id == device.id,
            None => device.is_default,
        };
        let playback = playback.clone();
        let settings = settings.clone();
        let id = device.id.clone();
        menu = menu.item(
            crate::common::menu::long_item(mark_if(selected, &device.name)).on_click(
                move |_, _, cx| {
                    let chosen = id.clone();
                    playback.update(cx, |playback, cx| {
                        playback.set_device(Some(chosen.clone()), cx);
                    });
                    settings.update(cx, |settings, cx| {
                        settings.set_preferred_device(Some(chosen), cx);
                    });
                },
            ),
        );
    }

    menu
}

impl Render for BottomBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (show_mini, stacked, left_width) = {
            let shell = self.shell.read(cx);
            (
                shell.shows_mini_now_playing(),
                shell.stacks_bottom_bar(),
                shell.bottom_left_width(),
            )
        };
        let (shuffle, repeat, count) = {
            let library = self.library.read(cx);
            (library.is_shuffle(), library.repeat_state(), library.len())
        };

        let bar = div()
            .flex_none()
            .w_full()
            .h(px(theme::bottom_bar_height(stacked)))
            .bg(theme::chrome());

        // 曲库为空时三个控件仍常驻、只降级为禁用态：整栏消失会让布局跳动。
        let enabled = count > 0;

        // 随机 / 循环按钮由 `mode_group` 现场构造而不是预造一份：`AnyElement` 不是 `Clone`，
        // 而「三列」与「两行」两种布局都要各用一次。

        let panel = self.shell.read(cx).right_panel();
        let now_playing_open = panel == Some(PanelTab::NowPlaying);
        let queue_open = panel == Some(PanelTab::Queue);
        let lyrics_open = self.shell.read(cx).shows_lyrics();

        // 全屏项还没有后端支撑，仍是明确的禁用态占位。
        let right = |fixed_width: Option<f32>| {
            let now_playing =
                crate::common::button::IconButton::new("bar-now-playing", Glyph::PanelRight)
                    .label(if enabled {
                        "正在播放视图"
                    } else {
                        "先导入音乐才能查看"
                    })
                    .box_size(28.)
                    .glyph_size(15.)
                    .color(if now_playing_open {
                        theme::text()
                    } else {
                        theme::text_muted()
                    })
                    .enabled(enabled)
                    .on_click({
                        let shell = self.shell.clone();
                        move |_, _, cx| {
                            shell.update(cx, |shell, cx| {
                                shell.toggle_panel(PanelTab::NowPlaying, cx);
                            });
                        }
                    })
                    .render();

            let lyrics = crate::common::button::IconButton::new("bar-lyrics", Glyph::Lyrics)
                .label(if lyrics_open {
                    "关闭歌词"
                } else {
                    "歌词"
                })
                .box_size(28.)
                .glyph_size(15.)
                .color(if lyrics_open {
                    theme::text()
                } else {
                    theme::text_muted()
                })
                .enabled(enabled)
                .on_click({
                    let shell = self.shell.clone();
                    move |_, _, cx| {
                        shell.update(cx, ShellEntity::toggle_lyrics);
                    }
                })
                .render();

            let queue = crate::common::button::IconButton::new("bar-queue", Glyph::Queue)
                .label(if enabled {
                    "播放队列"
                } else {
                    "先导入音乐才能查看队列"
                })
                .box_size(28.)
                .glyph_size(15.)
                .color(if queue_open {
                    theme::text()
                } else {
                    theme::text_muted()
                })
                .enabled(enabled)
                .on_click({
                    let shell = self.shell.clone();
                    move |_, _, cx| {
                        shell.update(cx, |shell, cx| shell.toggle_panel(PanelTab::Queue, cx));
                    }
                })
                .render();

            let devices = crate::common::menu::dropdown(
                crate::common::button::IconButton::new("bar-devices", Glyph::Devices)
                    .label("播放设备")
                    .box_size(28.)
                    .glyph_size(15.)
                    .render(),
                Anchor::TopRight,
                {
                    let playback = self.playback.clone();
                    let settings = self.settings.clone();
                    move |menu, _window, cx| device_menu(menu, &playback, &settings, cx)
                },
            );
            let fullscreen =
                crate::common::button::IconButton::new("bar-fullscreen", Glyph::Fullscreen)
                    .label("全屏（即将推出）")
                    .box_size(28.)
                    .glyph_size(15.)
                    .enabled(false)
                    .render();

            let mut group = div()
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(theme::GAP_XS))
                .child(now_playing)
                .child(lyrics)
                .child(queue)
                .child(devices)
                .child(self.volume.clone())
                .child(fullscreen);

            if let Some(width) = fixed_width {
                group = group.w(px(width));
            }
            group
        };

        if stacked {
            return bar
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(theme::GAP_XS))
                .px(px(theme::GAP_LG))
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(theme::GAP_LG))
                        .child(mode_group(
                            self.transport.clone(),
                            &self.library,
                            shuffle,
                            repeat,
                            enabled,
                        ))
                        .child(right(None)),
                )
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .min_w_0()
                        .child(self.seek.clone()),
                )
                .into_any_element();
        }

        let mut row = bar
            .flex()
            .items_center()
            .gap(px(theme::GAP_LG))
            .px(px(theme::GAP_LG));

        // 左栏（`ShellEntity::bottom_left_width`）始终存在；宽度与内容是两件事，`show_mini` 只管要不要往里放东西。
        row = row.child(
            div()
                .flex_none()
                .w(px(left_width))
                .min_w_0()
                .when(show_mini, |col| col.child(self.mini.clone())),
        );

        let center = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(theme::GAP_XS))
            .child(mode_group(
                self.transport.clone(),
                &self.library,
                shuffle,
                repeat,
                enabled,
            ))
            .child(
                div()
                    .w_full()
                    .max_w(px(theme::BOTTOM_CENTER_MAX))
                    .child(self.seek.clone()),
            );

        row.child(center)
            .child(right(Some(theme::BOTTOM_RIGHT_W)))
            .into_any_element()
    }
}

/// 传输区两侧的「随机 / 循环」按钮 + 中间的传输控件，合成一条操作带统一间距与基线。
fn mode_group(
    transport: Entity<Transport>,
    library: &Entity<LibraryEntity>,
    shuffle: bool,
    repeat: crate::entities::RepeatState,
    enabled: bool,
) -> AnyElement {
    let shuffle_button = crate::common::button::IconButton::new("bar-shuffle", Glyph::Shuffle)
        .label(if shuffle {
            "随机播放：开"
        } else {
            "随机播放：关"
        })
        .color(if shuffle {
            theme::text()
        } else {
            theme::text_muted()
        })
        .enabled(enabled)
        .on_click({
            let library = library.clone();
            move |_, _, cx| library.update(cx, LibraryEntity::toggle_shuffle)
        })
        .render();

    let repeat_glyph = match repeat {
        crate::entities::RepeatState::One => Glyph::RepeatOne,
        _ => Glyph::RepeatAll,
    };
    let repeat_active = repeat != crate::entities::RepeatState::Off;
    let repeat_button = crate::common::button::IconButton::new("bar-repeat", repeat_glyph)
        .label(if repeat_active {
            repeat.label()
        } else {
            "循环：关"
        })
        .color(if repeat_active {
            theme::text()
        } else {
            theme::text_muted()
        })
        .enabled(enabled)
        .on_click({
            let library = library.clone();
            move |_, _, cx| library.update(cx, LibraryEntity::cycle_repeat)
        })
        .render();

    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(theme::GAP_MD))
        .child(shuffle_button)
        .child(transport)
        .child(repeat_button)
        .into_any_element()
}
