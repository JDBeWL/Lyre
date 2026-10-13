//! 根视图的 `Render` 实现与元素树装配。
//! `read(cx)` 集中在 [`Frame::read`] 一次做完，其余函数只吃那份快照。

use std::time::Duration;

use crate::actions::*;
use crate::common::toast::ToastEntity;
use crate::components::{VolumeControl, detail};
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon};
use crate::platform::*;
use crate::shell::{NavSection, PanelTab, ShellEntity, SidebarState};
use crate::theme;

use super::RootView;
use super::actions::control_has_focus;
use super::nav::{back_tooltip, floating_back, toggle_settings};

// 步进量的权威在 `shortcuts.rs`，设置页的键位表显示的是同一批数字
const SEEK_STEP: Duration = Duration::from_secs(crate::shortcuts::SEEK_SECONDS);
const SEEK_STEP_LONG: Duration = Duration::from_secs(crate::shortcuts::SEEK_LONG_SECONDS);
const VOLUME_STEP: f32 = crate::shortcuts::VOLUME_STEP;
const PANEL_FADE: Duration = Duration::from_millis(160);
const DROP_FADE: Duration = Duration::from_millis(120);

// 元素 id 与键位上下文是命中测试 / 键位绑定的键，写错不报错
/// 键位上下文名。权威在 `shortcuts.rs`，`lyre-app` 的 `keymap.rs` 读的是同一个常量。
const KEY_CONTEXT: &str = crate::shortcuts::CONTEXT;
/// 搜索页探索区的滚动容器。
const SEARCH_BROWSE_SCROLL_ID: &str = "search-browse-scroll";
/// 拖拽导入遮罩。
const DROP_HINT_ID: &str = "drop-hint";
/// 遮罩淡入动画的 id，与元素 id 是两套 id 空间；字符串与 [`DROP_HINT_ID`] 相同只是巧合，不要合并。
const DROP_HINT_ANIM: &str = "drop-hint";

/// 一帧渲染要从各实体读出的快照。
/// 三个 `Entity` 按值存 —— `floating_back` 与元素树都要按值收句柄。
struct Frame {
    /// 右面板当前页签；`None` = 整块收起。
    right_panel: Option<PanelTab>,
    /// 歌词遮罩是否正盖在主区上。
    lyrics_open: bool,
    sidebar: SidebarState,
    nav: NavSection,
    /// 这一页画不画左上角的浮动返回按钮：只给「钻进去」的页面（作用域收窄、设置、搜索），一级区段走顶栏。
    /// 判据是历史栈而不是作用域 —— 从网格进专辑时作用域收窄了，但历史里记着网格。
    show_back: bool,
    /// 返回按钮的悬停提示：取历史栈顶（退到哪去），不是当前作用域。
    back_hint: SharedString,
    /// 页头色带种子：优先当前作用域，否则退回正在播放那首的专辑名。
    hero_seed: Option<String>,
    /// 底部提示条距窗口底的距离，按当前底栏高度算（窄窗口下底栏是两行、更高）。
    toast_bottom: f32,
    toast: Entity<ToastEntity>,
    library: Entity<LibraryEntity>,
    shell: Entity<ShellEntity>,
}

impl Frame {
    fn read(view: &RootView, cx: &App) -> Self {
        let shell = view.shell.read(cx);
        let right_panel = shell.right_panel();
        let lyrics_open = shell.shows_lyrics();
        let sidebar = shell.sidebar();
        let nav = shell.nav_section();
        let can_back = shell.can_go_back();
        let show_back = can_back
            && (view.library.read(cx).scope().is_scoped()
                || matches!(nav, NavSection::Search | NavSection::Settings));
        let toast_bottom = theme::bottom_bar_height(shell.stacks_bottom_bar()) + theme::GAP_LG;
        let back_hint =
            SharedString::from(back_tooltip(shell.peek_history().map(
                |(section, scope)| (section, view.library.read(cx).label_for(&scope)),
            )));

        let scoped = view.library.read(cx).scope_label();
        let hero_seed: Option<String> = scoped.clone().or_else(|| {
            view.playback
                .read(cx)
                .current()
                .and_then(|track| track.album.clone())
        });

        Self {
            right_panel,
            lyrics_open,
            sidebar,
            nav,
            show_back,
            back_hint,
            hero_seed,
            toast_bottom,
            toast: view.toast.clone(),
            library: view.library.clone(),
            shell: view.shell.clone(),
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = Frame::read(self, cx);

        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &TogglePlay, window, cx| {
                // gpui 内建会把 Enter / Space 转成聚焦控件的 click，这里再放一次就是一次按键干两件事
                if control_has_focus(window, cx) {
                    return;
                }
                this.toggle_play(cx);
            }))
            .on_action(cx.listener(|_this, _: &FocusNextControl, window, cx| {
                window.focus_next(cx);
            }))
            .on_action(cx.listener(|_this, _: &FocusPrevControl, window, cx| {
                window.focus_prev(cx);
            }))
            .on_action(cx.listener(|this, _: &NextTrack, _window, cx| {
                let step = this.library.update(cx, |library, _| library.step_next());
                this.apply_step(step, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevTrack, _window, cx| {
                let step = this.library.update(cx, |library, _| library.step_prev());
                this.apply_step(step, cx);
            }))
            .on_action(cx.listener(|this, _: &SeekForward, _window, cx| {
                this.nudge_seek(SEEK_STEP.as_millis() as i64, cx);
            }))
            .on_action(cx.listener(|this, _: &SeekBackward, _window, cx| {
                this.nudge_seek(-(SEEK_STEP.as_millis() as i64), cx);
            }))
            .on_action(cx.listener(|this, _: &SeekForwardLong, _window, cx| {
                this.nudge_seek(SEEK_STEP_LONG.as_millis() as i64, cx);
            }))
            .on_action(cx.listener(|this, _: &SeekBackwardLong, _window, cx| {
                this.nudge_seek(-(SEEK_STEP_LONG.as_millis() as i64), cx);
            }))
            .on_action(cx.listener(|this, _: &VolumeUp, _window, cx| {
                this.volume
                    .update(cx, |volume, cx| volume.nudge(VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &VolumeDown, _window, cx| {
                this.volume
                    .update(cx, |volume, cx| volume.nudge(-VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleMute, _window, cx| {
                this.volume.update(cx, VolumeControl::toggle_mute);
            }))
            .on_action(cx.listener(|this, _: &CyclePlayMode, _window, cx| {
                this.library.update(cx, LibraryEntity::cycle_mode);
            }))
            .on_action(cx.listener(|this, _: &SelectNext, _window, cx| {
                this.library.update(cx, LibraryEntity::focus_next);
            }))
            .on_action(cx.listener(|this, _: &SelectPrev, _window, cx| {
                this.library.update(cx, LibraryEntity::focus_prev);
            }))
            .on_action(cx.listener(|this, _: &FocusFirst, _window, cx| {
                this.library.update(cx, LibraryEntity::focus_first);
            }))
            .on_action(cx.listener(|this, _: &FocusLast, _window, cx| {
                this.library.update(cx, LibraryEntity::focus_last);
            }))
            .on_action(cx.listener(|this, _: &PlayFocused, window, cx| {
                // 与 `TogglePlay` 同理：焦点在控件上时 Enter 归那个控件
                if control_has_focus(window, cx) {
                    return;
                }
                // play_focused 只发 Play 事件，装载由 on_play 订阅完成
                this.library.update(cx, |library, cx| {
                    let _ = library.play_focused(cx);
                });
            }))
            .on_action(cx.listener(|this, _: &RemoveFocused, _window, cx| {
                this.remove_focused(cx);
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                // 聚焦的是顶栏那唯一的框，会触发 `SearchBoxEvent::Focused`，由订阅切到搜索页
                this.search
                    .update(cx, |search, cx| search.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                this.escape(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSettings, _window, cx| {
                let (shell, library) = (this.shell.clone(), this.library.clone());
                toggle_settings(&shell, &library, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleLyrics, _window, cx| {
                this.shell.update(cx, ShellEntity::toggle_lyrics);
            }))
            .on_action(cx.listener(|this, _: &ToggleShuffle, _window, cx| {
                this.library.update(cx, LibraryEntity::toggle_shuffle);
            }))
            // 按载荷类型筛：列表内拖拽排序也会置起 `active_drag`，
            // 用 `has_active_drag()` 判就会在排歌序时弹出导入遮罩
            .on_drag_move::<ExternalPaths>(cx.listener(
                |this, _: &DragMoveEvent<ExternalPaths>, _window, cx| {
                    if !this.drop_active {
                        this.drop_active = true;
                        cx.notify();
                    }
                },
            ))
            .on_file_drop_exit(cx.listener(|this, _event: &FileDropEvent, _window, cx| {
                if this.drop_active {
                    this.drop_active = false;
                    cx.notify();
                }
            }))
            // Submit 之前不一定有 Exited，这里自己清标志位，否则提示一直挂着
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                this.drop_active = false;
                let paths = paths.paths().to_vec();
                this.library
                    .update(cx, |library, cx| library.import(paths, cx));
                cx.notify();
            }))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::window_base())
            .text_color(theme::text())
            .child(self.top_bar.clone())
            .child(self.main_content(&frame, cx))
            .child(self.bottom_bar.clone())
            // 浮层排在最后：后画的在上面
            .children(self.overlays(&frame))
    }
}

impl RootView {
    /// 顶栏之下、底栏之上的主区：侧栏 + 中间卡片 +（可选）右面板。
    fn main_content(&self, frame: &Frame, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .px(px(theme::GUTTER))
            .pt(px(theme::GUTTER))
            .gap(px(theme::GUTTER))
            // 这里不再包圆角裁剪：外层 overflow_hidden 会把侧栏内部两块卡片的圆角一起裁平
            .when(frame.sidebar != SidebarState::Hidden, |row| {
                row.child(self.sidebar.clone())
            })
            .child(self.center_card(frame, cx))
            .when_some(frame.right_panel, |main, tab| {
                main.child(self.right_panel(tab))
            })
    }

    /// 中间那张卡片：页头色带 +（可选）浮动返回按钮 + 当前区段内容 +（可选）歌词遮罩。
    fn center_card(&self, frame: &Frame, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .rounded(px(theme::RADIUS_CARD))
            .overflow_hidden()
            .bg(theme::bg())
            .relative()
            // 排在最前面所以画在最底层，内容叠上去；空曲库时 hero_band 退回中性灰
            .when(frame.nav != NavSection::Settings, |card| {
                card.child(crate::common::gradient::hero_band(
                    frame.hero_seed.as_deref(),
                ))
            })
            .when(frame.show_back, |main| {
                main.child(floating_back(
                    frame.back_hint.clone(),
                    frame.shell.clone(),
                    frame.library.clone(),
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.nav_content(frame, cx)),
            )
            // 遮罩排在卡片内容最后：后画的在上面
            .when(frame.lyrics_open, |card| card.child(self.lyrics.clone()))
    }

    /// 主区内容按当前区段分派。
    fn nav_content(&self, frame: &Frame, cx: &mut Context<Self>) -> AnyElement {
        match frame.nav {
            NavSection::Home => div()
                .flex_1()
                .min_h_0()
                .child(self.home.clone())
                .into_any_element(),
            NavSection::Search => {
                // 只有有查询词且命中曲目时才挂「歌曲」结果列表，否则整页由探索区自己滚动
                let has_query = App::update_entity(cx, &self.library, |library, _| {
                    (library.has_query(), library.visible_len())
                });
                if has_query.0 && has_query.1 > 0 {
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(self.search_page.clone())
                        .child(div().flex_1().min_h_0().child(self.track_list.clone()))
                        .into_any_element()
                } else {
                    div()
                        .id(SEARCH_BROWSE_SCROLL_ID)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(self.search_page.clone())
                        .into_any_element()
                }
            }
            NavSection::Tracks => {
                // hero_for 找不到对应分组时（恢复历史带来的旧名字、已删除的歌单）退回纯列表。
                // 整个 Scope 传进去：种类与标签分开传会在同名专辑 / 艺人上拿错头图
                let hero = {
                    let scope = self.library.read(cx).scope().clone();
                    App::update_entity(cx, &self.library, |library, _| {
                        detail::hero_for(library, &scope)
                    })
                };
                let mut col = div().flex_1().min_h_0().flex().flex_col();
                if let Some(info) = hero {
                    col = col.child(detail::hero(
                        &info,
                        self.library.clone(),
                        &self.covers,
                        self.shell.clone(),
                        cx,
                    ));
                }
                col.child(div().flex_1().min_h_0().child(self.track_list.clone()))
                    .into_any_element()
            }
            NavSection::Albums | NavSection::Artists => div()
                .flex_1()
                .min_h_0()
                .child(self.album_grid.clone())
                .into_any_element(),
            NavSection::Settings => div()
                .flex_1()
                .min_h_0()
                .child(self.settings_panel.clone())
                .into_any_element(),
        }
    }

    /// 右侧面板那张卡片。只做出现不做收起：收起要求元素在状态变 `None` 后继续存活。
    fn right_panel(&self, tab: PanelTab) -> impl IntoElement {
        let panel = match tab {
            PanelTab::NowPlaying => self.now_playing.clone().into_any_element(),
            PanelTab::Queue => self.queue_panel.clone().into_any_element(),
        };
        // id 带页签名：切页签会重播，只改内容不重播
        let id = SharedString::from(format!("panel-{}", panel_id(tab)));
        div()
            .id(id.clone())
            .flex_none()
            .h_full()
            .rounded(px(theme::RADIUS_CARD))
            .overflow_hidden()
            .child(panel)
            // `with_animation` 必须放在链式末尾 —— 它返回的 AnimationElement<Div> 不再转发 Styled 的方法
            .with_animation(
                id,
                Animation::new(PANEL_FADE).with_easing(ease_out_quint()),
                Styled::opacity,
            )
    }

    /// 浮在整棵树之上的两层：拖拽导入遮罩与底部提示条。
    /// 返回 `Vec` 而不是包一层 `div()`：多一个 flex 子项会把根布局挤歪。
    fn overlays(&self, frame: &Frame) -> Vec<AnyElement> {
        let mut layers = Vec::new();
        if self.drop_active {
            layers.push(drop_hint().into_any_element());
        }
        layers.push(
            div()
                .absolute()
                .bottom(px(frame.toast_bottom))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(frame.toast.clone())
                .into_any_element(),
        );
        layers
    }
}

/// 面板的元素 id 后缀。动画 id 靠它区分「换了页签」和「同一页签内容变了」。
fn panel_id(tab: PanelTab) -> &'static str {
    match tab {
        PanelTab::NowPlaying => "now-playing",
        PanelTab::Queue => "queue",
    }
}

/// 只在拖拽期间存在于元素树：常驻但靠透明度隐藏的遮罩仍会吃掉整窗点击（透明度不参与命中测试）。
fn drop_hint() -> impl IntoElement {
    div()
        .id(DROP_HINT_ID)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x000000d9))
        .child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(theme::GAP_MD))
                .w(px(420.))
                .h(px(240.))
                .rounded(px(theme::RADIUS_XL))
                .border_2()
                .border_color(theme::control())
                .bg(theme::panel())
                .child(icon(Glyph::Import, 40., theme::text()))
                .child(
                    div()
                        .text_size(px(theme::FONT_SUBTITLE))
                        .text_color(theme::text())
                        .child("松手导入音频文件或文件夹"),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(theme::text_muted())
                        .child("支持 MP3 / FLAC / M4A / WAV / OGG / Opus 等"),
                ),
        )
        .with_animation(
            DROP_HINT_ANIM,
            Animation::new(DROP_FADE).with_easing(ease_out_quint()),
            Styled::opacity,
        )
}
