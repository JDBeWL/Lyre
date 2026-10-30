//! 导入流程：开对话框 → 起扫描线程 → 分批读标签 → 入队。
//! 扫描在后台线程跑，主线程每 `SCAN_FLUSH_INTERVAL` 收一批、每拍只 `notify` 一次。

use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use lyre_core::{Track, collect_audio_paths_with, scan_into_tracks};

use crate::platform::*;

use super::{LibraryEntity, LibraryEvent};

/// 扫描过程中每批之间回主线程的间隔。
const SCAN_FLUSH_INTERVAL: Duration = Duration::from_millis(80);
/// 每读多少首回传一批。
const SCAN_CHUNK: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanProgress {
    pub found: usize,
}

impl LibraryEntity {
    pub fn set_recursive(&mut self, recursive: bool, cx: &mut Context<Self>) {
        if self.recursive != recursive {
            self.recursive = recursive;
            cx.notify();
        }
    }

    pub fn recursive(&self) -> bool {
        self.recursive
    }

    pub fn scanning(&self) -> Option<ScanProgress> {
        self.scanning
    }

    pub fn open_dialog(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(SharedString::from("选择音频文件或文件夹")),
        });

        cx.spawn(async move |this, cx| {
            // 外层 Result 是「对话框能不能开」，内层是「用户选了什么」
            let Ok(result) = receiver.await else {
                return;
            };
            match result {
                Ok(Some(paths)) if !paths.is_empty() => {
                    this.update(cx, |this, cx| this.import(paths, cx)).ok();
                }
                Ok(_) => {}
                Err(err) => {
                    let message = format!("打开文件对话框失败：{err}");
                    this.update(cx, |_, cx| cx.emit(LibraryEvent::Failed(message)))
                        .ok();
                }
            }
        })
        .detach();
    }

    /// 导入一批路径（对话框或拖放）。扫描在后台线程跑、分批回传。
    // `_scan` 只写不读，作用是在 Drop 时取消扫描。
    #[allow(clippy::used_underscore_binding)]
    pub fn import(&mut self, inputs: Vec<PathBuf>, cx: &mut Context<Self>) {
        if inputs.is_empty() {
            return;
        }
        self.scanning = Some(ScanProgress::default());
        cx.notify();

        let (tx, rx) = mpsc::channel::<Vec<Track>>();
        // `background_executor()` 返回借用，要 clone 出所有权值才能带进 'static 任务
        let executor = cx.background_executor().clone();
        // 后台任务拿不到实体，开关按值带进去
        let recursive = self.recursive;

        // 展开目录 → 读标签 → 分批发送；`tx` drop 后的 `Disconnected` 就是扫完了。
        // 用专用 OS 线程而不是 `executor.spawn`：整段阻塞、没有一个 `await`，会长时间占住共享池里的一个 worker。
        let scan = std::thread::Builder::new()
            .name("mp-scan".to_owned())
            .spawn(move || {
                let files = collect_audio_paths_with(&inputs, recursive);
                scan_into_tracks(files, SCAN_CHUNK, |chunk| {
                    let _ = tx.send(chunk);
                });
            });

        if let Err(err) = scan {
            // 线程起不来（资源耗尽）：必须清掉 `scanning`，否则界面永远停在「正在扫描」；还要发 `Failed`，否则点了导入毫无反应。
            let message = format!("无法启动扫描线程：{err}");
            log::error!("{message}");
            self.scanning = None;
            cx.emit(LibraryEvent::Failed(message));
            cx.notify();
            return;
        }

        let task = cx.spawn(async move |this, cx| {
            // 跨轮次累计：`scanned` 数「扫到了几首」，`added` 数「其中新入库几首」；只报 `added` 的话空文件夹和全都已在库都是 0。
            let mut scanned = 0usize;
            let mut added = 0usize;
            loop {
                executor.timer(SCAN_FLUSH_INTERVAL).await;

                let mut batch = Vec::new();
                let mut done = false;
                loop {
                    match rx.try_recv() {
                        Ok(chunk) => batch.extend(chunk),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            done = true;
                            break;
                        }
                    }
                }

                if batch.is_empty() && !done {
                    continue;
                }

                let alive = this
                    .update(cx, |this, cx| {
                        if !batch.is_empty() {
                            // 先数再 `push`：它按值吃掉 `batch`。
                            let batch_len = batch.len();
                            scanned += batch_len;
                            let batch_added = this.playlist.push(batch);
                            added += batch_added;
                            // 重导只刷新标签时也要失效分组与检索字段缓存，否则搜索还按旧标题匹配。
                            this.invalidate();
                            if let Some(progress) = this.scanning.as_mut() {
                                progress.found += batch_added;
                            }
                            // 带过滤条件时新曲目也要按条件进投影，否则导入时列表看着毫无变化
                            if added > 0 && this.is_filtering() {
                                this.recompute_visible(cx);
                            }
                        }
                        if done {
                            this.scanning = None;
                            // 重导也会刷新已有条目的标签（`push` 对同 id 是替换），所以哪怕 added 为 0 也要写一次盘。
                            this.persist();
                            // 进度条消失是唯一的结束信号，空文件夹时更是全程无反馈，所以这里必须发事件。
                            cx.emit(LibraryEvent::Imported { scanned, added });
                        }
                        cx.notify();
                    })
                    .is_ok();

                if !alive || done {
                    break;
                }
            }
        });

        self._scan = Some(task);
    }
}
