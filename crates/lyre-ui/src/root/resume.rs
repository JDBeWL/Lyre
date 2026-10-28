//! `resume.json` 的读写策略：什么时候写、什么时候装回来。存盘形状在 [`crate::resume`]。

use std::time::Duration;

use crate::components::SettingsEntity;
use crate::entities::{LibraryEntity, PlaybackEntity};
use crate::platform::*;

/// 播放中兜底写一次 `resume.json` 的间隔。强杀和崩溃拿不到退出回调，损失得限制在一格以内。
pub(super) const RESUME_INTERVAL: Duration = Duration::from_secs(15);

/// 把「正在播哪首、播到哪」写进 `resume.json`。设置项关着就一个字都不写。
pub(super) fn save_resume(
    playback: &Entity<PlaybackEntity>,
    settings: &Entity<SettingsEntity>,
    cx: &mut App,
) {
    if !settings.read(cx).settings().resume_position {
        return;
    }
    let playback = playback.read(cx);
    let Some(track) = playback.current() else {
        return;
    };
    let position_ms = u64::try_from(playback.position().as_millis()).unwrap_or(u64::MAX);
    crate::resume::Resume::new(track.path.clone(), position_ms).save();
}

/// 启动时把上次那首装回来、停在上次那一秒，不自动开播。
pub(super) fn restore_resume(
    playback: &Entity<PlaybackEntity>,
    library: &Entity<LibraryEntity>,
    cx: &mut App,
) {
    let Some(resume) = crate::resume::Resume::load() else {
        return;
    };
    let track = library
        .read(cx)
        .tracks()
        .iter()
        .find(|track| track.path == resume.path)
        .cloned();
    let Some(track) = track else {
        // 文件被移走或改名，这条记录留着也没用了
        crate::resume::Resume::clear();
        return;
    };
    let id = track.id;
    let position = Duration::from_millis(resume.position_ms);
    // 静默恢复：订阅此时已经建立，普通 `load` 会在播放实体被可变借用时触发根视图回调。
    playback.update(cx, |playback, cx| playback.restore(track, position, cx));
    // 列表里那一行要亮起来，否则「界面在放、列表一行都不亮」
    library.update(cx, |library, cx| library.set_now_playing(Some(id), cx));
}
