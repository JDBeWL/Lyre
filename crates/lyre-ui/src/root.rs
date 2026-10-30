//! 根视图只做布局与装配。gpui 的脏标记打到所有祖先：进度通知会连带重绘整棵树（实测约 10% 单核）。
//! 不 `observe` 播放实体只省掉根视图自己那份重算，高频组件各自订阅 `PlaybackEntity`。

use std::cell::Cell;
use std::rc::Rc;

use crate::common::toast::ToastEntity;
use crate::components::{
    AlbumGrid, BottomBar, GridEvent, HomeView, LyricsView, MiniNowPlaying, MiniPlayerEvent,
    NowPlaying, QueuePanel, SearchBox, SearchBoxEvent, SearchPage, SeekBar, SettingsEntity,
    SettingsPanel, Sidebar, TopBar, TrackList, TrackListEvent, Transport, TransportEvent,
    VolumeControl,
};
use crate::cover::CoverCache;
// `Scope` 必须显式导入：`platform::*` 里带着 `gpui::Scope<'_>`，靠 glob 引进来会撞名。
use crate::entities::{LibraryEntity, LibraryEvent, PlaybackEntity, PlaybackEvent, Scope};
use crate::platform::*;
use crate::settings::Settings;
use crate::shell::{NavSection, PanelTab, ShellEntity};

mod actions;
mod nav;
mod render;
mod resume;

pub use nav::{
    BACK_BTN, BACK_INSET_TOP, HEADER_BACK_INSET, NavDirection, nav_click, toggle_settings,
};

pub struct RootView {
    playback: Entity<PlaybackEntity>,
    library: Entity<LibraryEntity>,
    shell: Entity<ShellEntity>,
    volume: Entity<VolumeControl>,
    search: Entity<SearchBox>,
    now_playing: Entity<NowPlaying>,
    /// 与 `now_playing` 二选一，由外壳决定。
    queue_panel: Entity<QueuePanel>,
    track_list: Entity<TrackList>,
    album_grid: Entity<AlbumGrid>,
    home: Entity<HomeView>,
    /// 歌词遮罩：自己 observe 播放实体，根视图只负责在打开时挂上去。
    lyrics: Entity<LyricsView>,
    /// 输入状态是 `search` 另建的一份，两者内容同步。
    search_page: Entity<SearchPage>,
    sidebar: Entity<Sidebar>,
    bottom_bar: Entity<BottomBar>,
    top_bar: Entity<TopBar>,
    settings_panel: Entity<SettingsPanel>,
    /// 只在落盘前读「恢复播放位置」开关，不 observe。
    settings: Entity<SettingsEntity>,
    toast: Entity<ToastEntity>,
    focus: FocusHandle,
    /// 由 `render` 的拖拽回调读写 —— gpui 会把外部拖拽的 `Entered/Pending` 翻译成一次 `MouseMove`。
    drop_active: bool,
    /// 播放中兜底写 `resume.json` 的节拍任务，持有它才不会被提前回收。
    _resume_watcher: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 先读盘：下面的实体按它初始化
        let stored = Settings::load();

        let playback = cx.new(|cx| PlaybackEntity::new(stored.preferred_device.clone(), cx));
        let library = cx.new(LibraryEntity::new);
        let shell = cx.new(|cx| ShellEntity::new(window, cx));
        let covers = cx.new(|_| CoverCache::default());
        let settings = cx.new(|cx| SettingsEntity::new(stored.clone(), cx));

        // 偏好要在启动时显式应用进 shell / library，只存着不会生效
        shell.update(cx, |shell, cx| shell.apply_settings(&stored, cx));
        library.update(cx, |library, cx| {
            library.set_recursive(stored.recursive_import, cx);
        });
        // 静音退出时以 0 起播，但存档里留的是静音前的值（见 `VolumeControl::persist`）
        playback.update(cx, |playback, cx| {
            // 输出模式先定：它决定后面第一次开流走哪条后端
            playback.set_exclusive_output(stored.exclusive_output);
            playback.set_volume(if stored.muted { 0.0 } else { stored.volume }, cx);
            playback.set_smooth_switch(stored.smooth_device_switch);
        });

        // 系统文件对话框属于另一个进程，截图脚本点不动；设了这个环境变量就从该目录直接导入。
        if let Some(dir) = std::env::var_os("LYRE_DEBUG_IMPORT") {
            let dir = std::path::PathBuf::from(dir);
            if dir.is_dir() {
                library.update(cx, |library, cx| library.import(vec![dir], cx));
            } else {
                // `PathBuf` 自己不带 `Display`，用 `Path::display()` 得到人类可读路径
                log::debug!(
                    "LYRE_DEBUG_IMPORT 指向的不是目录，已忽略：{}",
                    dir.display()
                );
            }
        }

        let seek_bar = cx.new(|cx| SeekBar::new(playback.clone(), cx));
        let volume = cx.new(|cx| VolumeControl::new(playback.clone(), settings.clone(), cx));

        let transport = cx.new(|cx| Transport::new(playback.clone(), cx));
        let now_playing =
            cx.new(|cx| NowPlaying::new(playback.clone(), covers.clone(), library.clone(), cx));
        let mini =
            cx.new(|cx| MiniNowPlaying::new(playback.clone(), covers.clone(), library.clone(), cx));
        let search_page =
            cx.new(|cx| SearchPage::new(library.clone(), shell.clone(), covers.clone(), cx));
        let track_list = cx.new(|cx| {
            TrackList::new(
                library.clone(),
                shell.clone(),
                covers.clone(),
                search_page.clone(),
                cx,
            )
        });
        let album_grid =
            cx.new(|cx| AlbumGrid::new(library.clone(), shell.clone(), covers.clone(), cx));
        let search = cx.new(|cx| SearchBox::new(window, library.clone(), cx));
        let sidebar = cx.new(|cx| Sidebar::new(shell.clone(), library.clone(), covers.clone(), cx));
        let home = cx.new(|cx| HomeView::new(shell.clone(), library.clone(), covers.clone(), cx));
        let lyrics = cx.new(|cx| LyricsView::new(playback.clone(), shell.clone(), cx));
        let queue_panel = cx.new(|cx| QueuePanel::new(library.clone(), cx));
        let settings_panel = cx.new(|cx| {
            SettingsPanel::new(
                settings.clone(),
                shell.clone(),
                library.clone(),
                playback.clone(),
                cx,
            )
        });
        let bottom_bar = cx.new(|cx| {
            BottomBar::new(
                shell.clone(),
                library.clone(),
                playback.clone(),
                settings.clone(),
                mini.clone(),
                transport.clone(),
                seek_bar.clone(),
                volume.clone(),
                cx,
            )
        });
        let top_bar = cx.new(|cx| TopBar::new(shell.clone(), library.clone(), search.clone(), cx));
        let toast = cx.new(ToastEntity::new);

        let on_playback = cx.subscribe(&playback, |this, _, event: &PlaybackEvent, cx| {
            match event {
                PlaybackEvent::Finished => {
                    let step = this.library.update(cx, |library, _| library.step_on_end());
                    this.apply_step(step, cx);
                }
                // `load` 是唯一的装载口，续播、点歌、队列跳转都会走到这里
                PlaybackEvent::TrackChanged => {
                    let played = this.playback.read(cx).current().map(|track| track.id);
                    if let Some(id) = played {
                        let library = this.library.clone();
                        library.update(cx, |library, cx| library.record_played(id, cx));
                    }
                    resume::save_resume(&this.playback, &this.settings, cx);
                }
                PlaybackEvent::StateChanged => {
                    resume::save_resume(&this.playback, &this.settings, cx);
                }
                // 输出模式与设备列表由设置面板、底栏各自订阅刷新
                PlaybackEvent::VolumeChanged
                | PlaybackEvent::DevicesChanged
                | PlaybackEvent::OutputModeChanged => {}
            }
        });

        let on_play = cx.subscribe(&library, |this, _, event: &LibraryEvent, cx| {
            match event {
                LibraryEvent::Play(track) => {
                    this.playback
                        .update(cx, |playback, cx| playback.load(track.clone(), true, cx));
                }
                // 头图是 RootView 内联渲染的（不 observe 曲库），改关注时没有别的重绘来源。
                LibraryEvent::FollowChanged => cx.notify(),
                // 换作用域后遮罩会把新内容整个盖住
                LibraryEvent::ScopeChanged => {
                    this.shell.update(cx, ShellEntity::close_lyrics);
                }
                // FocusMoved 由 TrackList 自己处理（滚动跟随）
                LibraryEvent::FocusMoved(_) => {}
                LibraryEvent::Imported { scanned, added } => {
                    let message = match (scanned, added) {
                        (0, _) => "这里没有找到音频文件".to_owned(),
                        // 扫到了但一首都没新增 = 整批都在库里，这次是刷新标签。
                        (s, 0) => format!("这 {s} 首已在曲库里，刷新了标签"),
                        (_, a) => format!("已导入 {a} 首"),
                    };
                    this.toast
                        .update(cx, |toast, cx| toast.show_plain(message, cx));
                }
                // 导入链路的失败：对话框开不了 / 扫描线程起不来
                LibraryEvent::Failed(message) => {
                    let message = message.clone();
                    this.toast
                        .update(cx, |toast, cx| toast.show_plain(message, cx));
                }
                // 曲库是后台读完的：拿到曲目后再装回上次播放位置。
                // 排到本次通知之后，让曲库自己的后续通知先落地。
                LibraryEvent::Loaded => {
                    if this.settings.read(cx).settings().resume_position {
                        let playback = this.playback.clone();
                        let library = this.library.clone();
                        cx.defer(move |cx| {
                            resume::restore_resume(&playback, &library, cx);
                        });
                    }
                }
            }
        });

        let on_transport =
            cx.subscribe(
                &transport,
                |this, _, event: &TransportEvent, cx| match event {
                    TransportEvent::TogglePlay => {
                        this.toggle_play(cx);
                    }
                    TransportEvent::Next => {
                        let step = this.library.update(cx, |library, _| library.step_next());
                        this.apply_step(step, cx);
                    }
                    TransportEvent::Prev => {
                        let step = this.library.update(cx, |library, _| library.step_prev());
                        this.apply_step(step, cx);
                    }
                },
            );

        let on_list_event = cx.subscribe(&track_list, |this, _, event: &TrackListEvent, cx| {
            match event.clone() {
                TrackListEvent::Removed(track, index) => {
                    this.offer_remove_undo(track, index, cx);
                }
                TrackListEvent::Cleared(tracks) => {
                    this.offer_clear_undo(tracks, cx);
                }
                // 必须在列表收窄作用域之前到达，所以列表是在 emit 之后才调 `enter_album`
                TrackListEvent::RecordFromTrack => {
                    this.record_return_point(cx);
                }
            }
        });

        let on_mini = cx.subscribe(&mini, |this, _, event: &MiniPlayerEvent, cx| {
            if matches!(event, MiniPlayerEvent::OpenNowPlaying) {
                this.shell.update(cx, |shell, cx| {
                    shell.toggle_panel(PanelTab::NowPlaying, cx);
                });
            }
        });

        // 顺序有意义：历史必须在收窄作用域之前记，否则记下的是「已经进了这张专辑」
        let on_grid = cx.subscribe(&album_grid, |this, _, event: &GridEvent, cx| {
            let from = this.shell.read(cx).nav_section();
            this.shell
                .update(cx, |shell, cx| shell.push_history(from, Scope::All, cx));

            match event.clone() {
                GridEvent::OpenAlbum(title) => {
                    this.library
                        .update(cx, |library, cx| library.enter_album(title, cx));
                }
                GridEvent::OpenArtist(name) => {
                    this.library
                        .update(cx, |library, cx| library.enter_artist(name, cx));
                }
            }
            this.shell
                .update(cx, |shell, cx| shell.set_nav(NavSection::Tracks, cx));
        });

        // 查询词的提交与防抖在 `SearchBox` 里；这里只切到搜索页，作用域保持不变
        let on_search = cx.subscribe(&search, |this, _, event: &SearchBoxEvent, cx| {
            if !matches!(event, SearchBoxEvent::Typing | SearchBoxEvent::Focused) {
                return;
            }
            if this.shell.read(cx).nav_section() != NavSection::Search {
                // 记一条历史，顶栏 ← 才能从搜索页退回来源页；沿用当前作用域。
                let here = (
                    this.shell.read(cx).nav_section(),
                    this.library.read(cx).scope().clone(),
                );
                this.shell.update(cx, |shell, cx| {
                    shell.push_history(here.0, here.1, cx);
                    shell.set_nav(NavSection::Search, cx);
                });
            }
        });

        let focus = cx.focus_handle();
        // 根视图拿到焦点，绑在 "Lyre" 上下文上的快捷键才会触发
        focus.focus(window, cx);

        // 兜底落盘：强杀与崩溃拿不到退出回调，播放中每 15 秒写一次，最多丢一格
        // 暂停时不写：那时 `StateChanged` 已经写过同一秒
        let resume_executor = cx.background_executor().clone();
        let (watch_playback, watch_settings) = (playback.clone(), settings.clone());
        let resume_watcher = cx.spawn_in(window, async move |this, cx| {
            loop {
                resume_executor.timer(resume::RESUME_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        if this.playback.read(cx).is_playing() {
                            resume::save_resume(&this.playback, &this.settings, cx);
                        }
                    })
                    .is_err()
                {
                    // 实体已销毁，收工，否则任务泄漏
                    break;
                }
            }
        });

        // 正常关窗走这一支：`Alt+F4` 是 WM_CLOSE，最后一个窗口关闭即 quit
        let quit_flushed = Rc::new(Cell::new(false));
        let on_quit = {
            let flushed = Rc::clone(&quit_flushed);
            cx.on_app_quit(move |_window, cx| {
                if !flushed.replace(true) {
                    log::info!("正在退出，刷写持久化队列");
                    resume::save_resume(&watch_playback, &watch_settings, cx);
                    // 后台写线程是异步的：关窗前同步等它把最后一格落盘。
                    crate::persist::flush();
                    let (writes, bytes) = crate::persist::write_stats();
                    log::info!("本次运行持久化写入 {writes} 次，共 {bytes} 字节");
                }
                async {}
            })
        };

        // 这个 gpui 版本关最后一个窗口不一定触发 `on_app_quit`，窗口关闭回调才是可靠兜底。
        let on_window_closed = {
            let flushed = Rc::clone(&quit_flushed);
            let root = cx.entity().downgrade();
            cx.on_window_closed(move |cx, _| {
                if flushed.replace(true) {
                    return;
                }
                if root
                    .update(cx, |this, cx| {
                        resume::save_resume(&this.playback, &this.settings, cx);
                    })
                    .is_err()
                {
                    log::warn!("关窗时根视图已销毁，跳过续播位置保存");
                }
                crate::persist::flush();
                let (writes, bytes) = crate::persist::write_stats();
                log::info!("关窗：本次运行持久化写入 {writes} 次，共 {bytes} 字节");
            })
        };

        Self {
            playback,
            library,
            shell,
            volume,
            search,
            now_playing,
            queue_panel,
            track_list,
            album_grid,
            home,
            lyrics,
            search_page,
            sidebar,
            bottom_bar,
            toast,
            top_bar,
            settings_panel,
            settings,
            focus,
            drop_active: false,
            _resume_watcher: resume_watcher,
            _subscriptions: vec![
                on_playback,
                on_play,
                on_transport,
                on_list_event,
                on_mini,
                on_grid,
                on_search,
                on_quit,
                on_window_closed,
            ],
        }
    }

    /// 命令行导入（`lyre.exe <文件或文件夹>...`），「关联文件类型」依赖它。
    pub fn import(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.library
            .update(cx, |library, cx| library.import(paths, cx));
    }
}
