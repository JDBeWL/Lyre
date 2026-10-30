//! Lyre 入口。
// 防止Windows下的panic弹窗挡住日志，panic信息无法收集，以及关闭release下的命令行窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod keymap;

use std::path::PathBuf;

use lyre_ui::RootView;
use lyre_ui::platform::*;

fn main() {
    application().run(|cx: &mut App| {
        // 日志必须先初始化
        lyre_ui::logging::init();
        // 组件库必须在开窗之前初始化
        lyre_ui::platform::init(cx);
        keymap::bind(cx);

        let bounds = Bounds::centered(None, size(px(1120.), px(740.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // 底栏左右两栏各 240px、进度条两端时间标签各 52px。
            window_min_size: Some(size(px(560.), px(400.))),
            titlebar: Some(TitlebarOptions {
                // `appears_transparent: true` 关掉系统标题栏（ONLY macOS / Windows），拖拽与窗口控件仍由 GPUI 应答
                // `title` 进任务栏 tooltip 与无障碍树的窗口名，留空会报出匿名窗口
                title: Some(SharedString::from("Lyre")),
                appears_transparent: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        // 开窗失败只记日志退出，不 panic：release 下没有控制台。
        if let Err(err) = cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| RootView::new(window, cx));

            // 命令行给的路径直接导入，也是「用本应用打开」的落点
            let initial: Vec<PathBuf> = std::env::args_os()
                .skip(1)
                .map(PathBuf::from)
                .filter(|path| path.exists())
                .collect();
            if !initial.is_empty() {
                view.update(cx, |view, cx| view.import(initial, cx));
            }

            // 窗口根必须包一层组件库的 Root，否则主题 / 浮层 / 焦点陷阱拿不到 Context
            lyre_ui::platform::rooted(view, window, cx)
        }) {
            log::error!("打开主窗口失败：{err}");
        }
    });
}
