//! 设置面板：每项平铺成「标题 + 一组互斥选项」，最多三个选项；另有只读的快捷键一览与数据统计。

use std::rc::Rc;

use crate::common::button::TAB_STOP;
use crate::common::thousands;
use crate::entities::{LibraryEntity, PlaybackEntity, PlaybackEvent};
use crate::icon::{Glyph, icon};
use crate::persist::data_dir;
use crate::platform::*;
use crate::settings::Settings;
use crate::shell::{Density, NavSection, PanelTab, ShellEntity, SidebarState};
use crate::theme;
use lyre_audio::OutputMode;

pub struct SettingsPanel {
    settings: Entity<SettingsEntity>,
    shell: Entity<ShellEntity>,
    library: Entity<LibraryEntity>,
    /// 音频相关的偏好改完要立刻下发，不能只等下次启动。
    playback: Entity<PlaybackEntity>,
    /// 快捷键卡的展开态，本地状态、不落盘。
    shortcuts_open: bool,
    /// 面板整块可纵向滚动，矮窗口下「导入」以下会被裁掉。
    /// gpui 不画滚动条，要自己叠一条（见 `common::scroll::scrollbar`）；句柄必须常驻，每帧新建等于归零。
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl SettingsPanel {
    pub fn new(
        settings: Entity<SettingsEntity>,
        shell: Entity<ShellEntity>,
        library: Entity<LibraryEntity>,
        playback: Entity<PlaybackEntity>,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = cx.observe(&settings, |_, _, cx| cx.notify());
        // 数据卡写着各项计数，所以要 observe 曲库；曲库仅在扫描时每 80ms notify 一批。
        let library_sub = cx.observe(&library, |_, _, cx| cx.notify());
        // 只关心输出模式：observe 播放实体会跟着进度的 10Hz 重绘一起白跑
        let mode_sub = cx.subscribe(&playback, |_, _, event: &PlaybackEvent, cx| {
            if matches!(event, PlaybackEvent::OutputModeChanged) {
                cx.notify();
            }
        });
        Self {
            settings,
            shell,
            library,
            playback,
            shortcuts_open: false,
            scroll: ScrollHandle::new(),
            _subs: vec![sub, library_sub, mode_sub],
        }
    }

    /// 独占协商到的口径。非独占或还没协商到时没有可显示的东西。
    fn output_detail(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let playback = self.playback.read(cx);
        if playback.output_mode() != OutputMode::Exclusive {
            return None;
        }
        let detail = playback.output_detail();
        let text = if detail.is_empty() {
            "正在协商独占格式…".to_owned()
        } else {
            format!("当前独占格式：{detail}")
        };
        Some(
            div()
                .text_size(px(theme::FONT_MICRO))
                .text_color(theme::text_muted())
                .child(text),
        )
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.settings.read(cx).settings();

        // `on_pick` 要求 `'static`，每个闭包各捕获一份句柄克隆，不能共用外层变量。
        let (for_density, for_view, for_recursive) = (
            self.settings.clone(),
            self.settings.clone(),
            self.settings.clone(),
        );
        let (for_panel, for_sidebar, for_resume) = (
            self.settings.clone(),
            self.settings.clone(),
            self.settings.clone(),
        );
        let (for_smooth, for_output) = (self.settings.clone(), self.settings.clone());
        let (smooth_player, output_player) = (self.playback.clone(), self.playback.clone());
        let shell = self.shell.clone();
        let library = self.library.clone();

        let density: Picker = Rc::new(move |index: usize, cx: &mut App| {
            let value = if index == 0 {
                Density::Compact
            } else {
                Density::Comfortable
            };
            // 既落盘供下次启动，也立刻应用到 shell，否则改完看不出变化。
            for_density.update(cx, |this, cx| this.set_density(value, cx));
            shell.update(cx, |shell, cx| shell.set_density(value, cx));
        });

        let view: Picker = Rc::new(move |index: usize, cx: &mut App| {
            let value = match index {
                1 => NavSection::Albums,
                2 => NavSection::Artists,
                _ => NavSection::Tracks,
            };
            for_view.update(cx, |this, cx| this.set_default_view(value, cx));
        });

        let recursive: Picker = Rc::new(move |index: usize, cx: &mut App| {
            let value = index == 0;
            for_recursive.update(cx, |this, cx| this.set_recursive_import(value, cx));
            library.update(cx, |library, cx| library.set_recursive(value, cx));
        });

        // 下面三项是启动时的偏好：改完不动当前界面，否则「收起右面板」会变成一条关不掉的设置。
        let panel: Picker = Rc::new(move |index: usize, cx: &mut App| {
            let value = match index {
                1 => Some(PanelTab::NowPlaying),
                2 => Some(PanelTab::Queue),
                _ => None,
            };
            for_panel.update(cx, |this, cx| this.set_default_panel(value, cx));
        });

        let sidebar: Picker = Rc::new(move |index: usize, cx: &mut App| {
            let value = match index {
                1 => Some(SidebarState::Expanded),
                2 => Some(SidebarState::Icons),
                _ => None,
            };
            for_sidebar.update(cx, |this, cx| this.set_default_sidebar(value, cx));
        });

        let resume: Picker = Rc::new(move |index: usize, cx: &mut App| {
            for_resume.update(cx, |this, cx| this.set_resume_position(index == 0, cx));
        });

        // 音频线程是另一条线程，落盘之外还得当场把新值送过去
        let smooth: Picker = Rc::new(move |index: usize, cx: &mut App| {
            let value = index == 0;
            for_smooth.update(cx, |this, cx| this.set_smooth_device_switch(value, cx));
            smooth_player.update(cx, |playback, _cx| playback.set_smooth_switch(value));
        });

        let output: Picker = Rc::new(move |index: usize, cx: &mut App| {
            let exclusive = index == 0;
            for_output.update(cx, |this, cx| this.set_exclusive_output(exclusive, cx));
            output_player.update(cx, |playback, _cx| playback.set_exclusive_output(exclusive));
        });

        // 两列的判断只读 `center_width()`，不在这里减侧栏与右面板。
        let (two_columns, back_inset) = {
            let shell = self.shell.read(cx);
            let width = shell.center_width();
            (
                width >= TWO_COL_MIN,
                // 内容占满一行才需要给浮动返回按钮让位；居中成窄栏时按钮落在左侧留白里
                shell.can_go_back() && width - 2.0 * theme::GAP_LG <= CONTENT_MAX_W,
            )
        };

        let list_card = card("列表").child(choice_row(
            cx,
            Glyph::List,
            "行密度",
            &[
                ("紧凑", settings.density == Density::Compact),
                ("舒适", settings.density == Density::Comfortable),
            ],
            &density,
        ));
        let startup_card = card("启动")
            .child(choice_row(
                cx,
                Glyph::Home,
                "默认视图",
                &[
                    ("曲目", settings.default_view == NavSection::Tracks),
                    ("专辑", settings.default_view == NavSection::Albums),
                    ("艺术家", settings.default_view == NavSection::Artists),
                ],
                &view,
            ))
            .child(choice_row(
                cx,
                Glyph::PanelRight,
                "右面板",
                &[
                    ("收起", settings.default_panel.is_none()),
                    (
                        "现在播放",
                        settings.default_panel == Some(PanelTab::NowPlaying),
                    ),
                    ("播放队列", settings.default_panel == Some(PanelTab::Queue)),
                ],
                &panel,
            ))
            .child(choice_row(
                cx,
                Glyph::PanelLeft,
                "侧栏",
                &[
                    ("跟随宽度", settings.default_sidebar.is_none()),
                    (
                        "总是展开",
                        settings.default_sidebar == Some(SidebarState::Expanded),
                    ),
                    (
                        "只用图标",
                        settings.default_sidebar == Some(SidebarState::Icons),
                    ),
                ],
                &sidebar,
            ))
            .child(choice_row(
                cx,
                Glyph::Clock,
                "上次播放",
                &[
                    ("接着播", settings.resume_position),
                    ("每次从头", !settings.resume_position),
                ],
                &resume,
            ));
        let import_card = card("导入").child(choice_row(
            cx,
            Glyph::Folder,
            "文件夹",
            &[
                ("含子目录", settings.recursive_import),
                ("仅顶层", !settings.recursive_import),
            ],
            &recursive,
        ));

        let audio_card = card("音频")
            .child(choice_row(
                cx,
                Glyph::VolumeHigh,
                "输出模式",
                &[
                    ("独占", settings.exclusive_output),
                    ("共享", !settings.exclusive_output),
                ],
                &output,
            ))
            .child(choice_row(
                cx,
                Glyph::Devices,
                "切换输出设备",
                &[
                    ("平滑过渡", settings.smooth_device_switch),
                    ("直接切换", !settings.smooth_device_switch),
                ],
                &smooth,
            ))
            .children(self.output_detail(cx));

        let left = vec![list_card, startup_card];
        let right = vec![audio_card, import_card, data_section(&self.library, cx)];
        let choices = if two_columns {
            div()
                .w_full()
                .flex()
                .gap(px(theme::GAP_LG))
                .child(column(left))
                .child(column(right))
                .into_any_element()
        } else {
            let mut stacked = left;
            stacked.extend(right);
            column(stacked)
        };

        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::GAP_LG))
            .p(px(theme::GAP_LG))
            // 右内边距多留一档：滚动条是叠在滚动区上的兄弟节点，不避让就压在数字和路径上
            .pr(px(theme::GAP_XL))
            .child(choices)
            .child(shortcuts_section(two_columns, self.shortcuts_open, cx));

        // 内容比窗口高时会被裁掉且够不着，所以整块可滚；`track_scroll` 同时是滚动条读偏移量的来源。
        let scroll_area = div()
            .id("settings-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .flex()
                    .justify_center()
                    .w_full()
                    .child(div().w_full().max_w(px(CONTENT_MAX_W)).child(body)),
            );

        div()
            .flex_1()
            .min_w_0()
            .w_full()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            // 不铺底色：铺一层不透明会把浮在左上角的返回按钮整个盖掉
            .child(header(back_inset))
            .child(scroll_area)
            .child(crate::common::scroll::scrollbar(
                &self.scroll,
                "settings-scrollbar",
            ))
    }
}

/// 宽到多少才排两列，窄一档回到单列。
const TWO_COL_MIN: f32 = 760.0;
/// 整块内容的上限宽度，再宽就居中留白。
const CONTENT_MAX_W: f32 = 1080.0;

/// 一张设置卡：`surface` 底色、`RADIUS_CARD` 圆角，分区标题在卡内顶部。
fn card(title: &str) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_MD))
        .p(px(theme::GAP_LG))
        .rounded(px(theme::RADIUS_CARD))
        .bg(theme::surface())
        .child(crate::common::section::section_heading(title))
}

/// 一列卡片。`flex_1` + `min_w_0`：长路径与长标题不许把某一列顶宽。
fn column(cards: Vec<Div>) -> AnyElement {
    cards
        .into_iter()
        .fold(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(theme::GAP_LG)),
            ParentElement::child,
        )
        .into_any_element()
}

/// 页头：大标题靠左、版本号靠右，装在一条与正文同宽的居中栏里（同为 `CONTENT_MAX_W`）。
/// 标题行顶到返回按钮那一行、与它等高；让位量与按钮位置同源于 `root::BACK_*`。
fn header(back_inset: bool) -> impl IntoElement {
    div().flex_none().w_full().flex().justify_center().child(
        div()
            .w_full()
            .max_w(px(CONTENT_MAX_W))
            .flex()
            .flex_col()
            .pt(px(if back_inset {
                crate::root::BACK_INSET_TOP
            } else {
                theme::GAP_XL
            }))
            .pb(px(theme::GAP_LG))
            .pl(px(if back_inset {
                // 让位量之外再留一档：按钮右缘恰好落在 `HEADER_BACK_INSET`，不留标题就贴着圆。
                crate::root::HEADER_BACK_INSET + theme::GAP_SM
            } else {
                theme::GAP_LG
            }))
            // 页头的右内边距跟正文一致，版本号才和卡片右缘齐。
            .pr(px(theme::GAP_XL))
            .child(
                div()
                    .h(px(crate::root::BACK_BTN))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(theme::FONT_HERO))
                            .font_weight(theme::WEIGHT_TITLE)
                            .text_color(theme::text())
                            .child("设置"),
                    )
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(theme::text_faint())
                            .child(concat!("v", env!("CARGO_PKG_VERSION"))),
                    ),
            ),
    )
}

/// 分区里的小标题（快捷键分三组，组名取自 `shortcuts::GROUPS`）。间距交给容器，这里不带 `mt`。
fn sub_title(text: &str) -> impl IntoElement {
    div()
        .text_size(px(theme::FONT_MICRO))
        .font_weight(theme::WEIGHT_BUTTON)
        .text_color(theme::text_faint())
        .child(text.to_owned())
}

/// 一行设置：`title` + 一组互斥选项。`on_pick` 收到选项下标，由调用方映射成枚举。
/// 回调签名压成 `Picker`：到处写全 `Rc<dyn Fn…>` 已跨过 clippy 的类型复杂度上限。
type Picker = Rc<dyn Fn(usize, &mut App)>;

fn choice_row(
    cx: &mut Context<SettingsPanel>,
    glyph: Glyph,
    title: &str,
    options: &[(&str, bool)],
    on_pick: &Picker,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_SM))
                .child(icon(glyph, 15., theme::text_muted()))
                .child(
                    div()
                        .text_size(px(theme::FONT_BODY))
                        .text_color(theme::text())
                        .child(title.to_owned()),
                ),
        )
        .child(segmented(cx, title, options, on_pick))
}

/// 分段控件：一个深色轨道内并排若干互斥格，选中的那格白底黑字。
/// 无障碍状态由 `aria_selected` 与提示里的「（已选中）」表达。
fn segmented(
    cx: &mut Context<SettingsPanel>,
    title: &str,
    options: &[(&str, bool)],
    on_pick: &Picker,
) -> Div {
    let mut track = div()
        .mt(px(theme::GAP_MD))
        .flex()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(theme::RADIUS_MD))
        .bg(theme::bg());

    for (index, (label, active)) in options.iter().enumerate() {
        let label = (*label).to_owned();
        let active = *active;
        let on_pick = Rc::clone(on_pick);
        let a11y = format!("{title}：{label}{}", if active { "（已选中）" } else { "" });
        let (base_bg, base_color) = if active {
            (theme::control(), theme::text_on_control())
        } else {
            (theme::transparent(), theme::text_muted())
        };

        track = track.child(
            div()
                .id(SharedString::from(format!("choice-{title}-{index}")))
                .flex()
                .items_center()
                .justify_center()
                .px(px(theme::GAP_MD))
                .py(px(5.))
                .rounded(px(theme::RADIUS_SM))
                .tab_index(TAB_STOP)
                .focus_visible(theme::focus_ring)
                .role(Role::Button)
                .aria_selected(active)
                .text_size(px(theme::FONT_CAPTION))
                .font_weight(theme::WEIGHT_BUTTON)
                .bg(base_bg)
                .text_color(base_color)
                .cursor_pointer()
                // 悬停只给未选项：选中态已是最亮档，再套 hover 会盖掉它的底色。
                .when(!active, |seg| {
                    seg.hover(|style| style.bg(theme::elevated()).text_color(theme::text()))
                })
                .on_click(cx.listener(move |_, _, _, cx| on_pick(index, cx)))
                .tooltip(crate::common::tooltip::builder(a11y))
                .child(label),
        );
    }

    track
}

/// 一行「名字 + 值」的只读信息。值靠右、过长时截断，免得长路径把这一行顶宽。
fn info_row(label: &str, value: impl Into<String>) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(theme::GAP_MD))
        .text_size(px(theme::FONT_MICRO))
        .child(
            div()
                .flex_none()
                .text_color(theme::text_muted())
                .child(label.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme::text())
                .text_right()
                .child(value.into()),
        )
}

/// 「数据」里的一格统计：大数字压着小标签。
fn stat_cell(label: &str, value: String) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(theme::FONT_SUBTITLE))
                .font_weight(theme::WEIGHT_TITLE)
                .text_color(theme::text())
                .child(value),
        )
        .child(
            div()
                .text_size(px(theme::FONT_MICRO))
                .text_color(theme::text_faint())
                .child(label.to_owned()),
        )
}

/// 快捷键卡：宽屏三组并排占满整行，窄屏回到一列。默认收起，`open` 由调用方传入。
fn shortcuts_section(two_columns: bool, open: bool, cx: &mut Context<SettingsPanel>) -> Div {
    let chevron = if open {
        Glyph::ChevronDown
    } else {
        Glyph::ChevronRight
    };
    let head = div()
        .id("shortcuts-toggle")
        .flex()
        .items_center()
        .gap(px(theme::GAP_SM))
        .cursor_pointer()
        .tab_index(TAB_STOP)
        .focus_visible(theme::focus_ring)
        .role(Role::Button)
        .aria_selected(open)
        .on_click(cx.listener(|this, _, _, cx| {
            this.shortcuts_open = !this.shortcuts_open;
            cx.notify();
        }))
        .tooltip(crate::common::tooltip::builder(if open {
            "收起快捷键"
        } else {
            "展开快捷键"
        }))
        .child(icon(chevron, 14., theme::text_muted()))
        .child(crate::common::section::section_heading("快捷键"));

    let mut card = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_MD))
        .p(px(theme::GAP_LG))
        .rounded(px(theme::RADIUS_CARD))
        .bg(theme::surface())
        .child(head);

    if !open {
        return card;
    }

    let mut groups = div()
        .w_full()
        .flex()
        .gap(px(theme::GAP_LG))
        .mt(px(theme::GAP_SM));
    if !two_columns {
        groups = groups.flex_col();
    }

    for (group, entries) in crate::shortcuts::GROUPS {
        let mut col = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(theme::GAP_XS));
        col = col.child(sub_title(group));
        for entry in *entries {
            col = col.child(info_row(entry.label, pretty_keys(entry.keys)));
        }
        groups = groups.child(col);
    }

    card = card.child(groups);
    card
}

fn data_section(library: &Entity<LibraryEntity>, cx: &App) -> Div {
    let library = library.read(cx);
    let counts = [
        ("曲目", library.len()),
        ("专辑", library.album_count()),
        ("艺术家", library.artist_count()),
        ("已收藏", library.liked_count()),
        ("已关注", library.followed_count()),
        ("最近播放", library.recent_count()),
        ("歌单", library.collections().len()),
    ];

    let mut grid = div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_MD))
        .mt(px(theme::GAP_SM));
    for pair in counts.chunks(2) {
        let mut row = div().flex().gap(px(theme::GAP_MD));
        for &(label, count) in pair {
            row = row.child(stat_cell(label, thousands(count)));
        }
        // 奇数项时补一个空位，否则末格被 flex_1 拉成整行宽。
        if pair.len() == 1 {
            row = row.child(div().flex_1().min_w_0());
        }
        grid = grid.child(row);
    }

    let mut content = div().flex().flex_col().child(grid);
    if let Some(dir) = data_dir() {
        content = content.child(
            div()
                .mt(px(theme::GAP_MD))
                .child(info_row("数据目录", dir.display().to_string())),
        );
    }

    card("数据").child(content)
}

/// 把内部写法 `ctrl-up` 翻成界面上的 `Ctrl + ↑`，只翻展示层。
/// `keymap.rs` 绑定的仍是原始串，两边都从 `crate::shortcuts::keys` 取值。
fn pretty_keys(raw: &str) -> String {
    raw.split('-')
        .map(|part| match part {
            "ctrl" => "Ctrl".to_owned(),
            "alt" => "Alt".to_owned(),
            "shift" => "Shift".to_owned(),
            "up" => "↑".to_owned(),
            "down" => "↓".to_owned(),
            "left" => "←".to_owned(),
            "right" => "→".to_owned(),
            "space" => "Space".to_owned(),
            "escape" => "Esc".to_owned(),
            "enter" => "Enter".to_owned(),
            "delete" => "Delete".to_owned(),
            "home" => "Home".to_owned(),
            "end" => "End".to_owned(),
            other if other.len() == 1 => other.to_uppercase(),
            other => other.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// 设置的持有者：改动一律经 `update_and_save` 落盘，绕过它就等于改了不保存。
pub struct SettingsEntity {
    settings: Settings,
}

impl SettingsEntity {
    pub fn new(settings: Settings, cx: &mut Context<Self>) -> Self {
        let _ = cx;
        Self { settings }
    }

    pub fn settings(&self) -> Settings {
        self.settings.clone()
    }

    fn update_and_save(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Settings)) {
        edit(&mut self.settings);
        self.settings.save();
        cx.notify();
    }

    pub fn set_density(&mut self, density: Density, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| settings.density = density);
    }

    pub fn set_default_view(&mut self, view: NavSection, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| settings.default_view = view);
    }

    pub fn set_recursive_import(&mut self, recursive: bool, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| settings.recursive_import = recursive);
    }

    pub fn set_default_panel(&mut self, panel: Option<PanelTab>, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| settings.default_panel = panel);
    }

    pub fn set_default_sidebar(&mut self, sidebar: Option<SidebarState>, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| settings.default_sidebar = sidebar);
    }

    pub fn set_resume_position(&mut self, resume: bool, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| settings.resume_position = resume);
        // 关掉就把文件删了，否则重新打开会跳回一个几个月前的位置
        if !resume {
            crate::resume::Resume::clear();
        }
    }

    /// 音量与静音必须一起写：静音时 `volume` 存的仍是静音前的值，分开写会留下「取消静音回到 0」的存档。
    pub fn set_volume(&mut self, volume: f32, muted: bool, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| {
            settings.volume = volume;
            settings.muted = muted;
        });
    }

    /// 记住输出设备。`None` 表示跟随系统默认。
    pub fn set_preferred_device(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| settings.preferred_device = device);
    }

    /// 记住换设备时是否做交叉淡化。
    pub fn set_smooth_device_switch(&mut self, smooth: bool, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| settings.smooth_device_switch = smooth);
    }

    /// 记住是否用 WASAPI 独占输出。
    pub fn set_exclusive_output(&mut self, exclusive: bool, cx: &mut Context<Self>) {
        self.update_and_save(cx, |settings| settings.exclusive_output = exclusive);
    }
}

#[cfg(test)]
mod tests {
    use super::pretty_keys;

    #[test]
    fn modifier_and_arrow_keys_read_as_ui_text() {
        assert_eq!(pretty_keys("ctrl-up"), "Ctrl + ↑");
        assert_eq!(pretty_keys("shift-left"), "Shift + ←");
        assert_eq!(pretty_keys("alt-right"), "Alt + →");
        assert_eq!(pretty_keys("space"), "Space");
        assert_eq!(pretty_keys("escape"), "Esc");
    }

    #[test]
    fn single_character_keys_survive_their_own_hyphen() {
        // `ctrl-,` 切出来是 ["ctrl", ","]，单字符要变大写而不是当键名查表
        assert_eq!(pretty_keys("ctrl-,"), "Ctrl + ,");
        assert_eq!(pretty_keys("/"), "/");
        assert_eq!(pretty_keys("m"), "M");
    }
}
