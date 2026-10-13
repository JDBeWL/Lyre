//! 底部控制栏：三列网格 —— 左曲目身份 / 中运输 + 进度 / 右模式 · 音量 · 队列。
//! 左右两栏都是显式宽度（左 16→240 随窗口连续变化，右固定 320），窄于 `STACKED_BOTTOM_MIN`（640px）时改两行、进度条独占一行。

use crate::components::mini_now_playing::MiniNowPlaying;
use crate::components::seek_bar::SeekBar;
use crate::components::transport::Transport;
use crate::components::volume::VolumeControl;
use crate::entities::LibraryEntity;
use crate::icon::Glyph;
use crate::platform::*;
use crate::shell::{PanelTab, ShellEntity};
use crate::theme;

pub struct BottomBar {
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
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
        mini: Entity<MiniNowPlaying>,
        transport: Entity<Transport>,
        seek: Entity<SeekBar>,
        volume: Entity<VolumeControl>,
        cx: &mut Context<Self>,
    ) -> Self {
        // 模式按钮的图标要跟着模式变，所以得 observe 曲库（低频，可接受）
        let library_sub = cx.observe(&library, |_, _, cx| cx.notify());
        let shell_sub = cx.observe(&shell, |_, _, cx| cx.notify());

        Self {
            shell,
            library,
            mini,
            transport,
            seek,
            volume,
            _subs: vec![library_sub, shell_sub],
        }
    }
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

        // 设备 / 全屏两项没有后端支撑，所以是明确的禁用态占位。
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

            let devices = crate::common::button::IconButton::new("bar-devices", Glyph::Devices)
                .label("播放设备（即将推出）")
                .box_size(28.)
                .glyph_size(15.)
                .enabled(false)
                .render();
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
