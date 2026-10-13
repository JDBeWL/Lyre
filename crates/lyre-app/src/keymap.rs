//! 全局快捷键绑定

use lyre_ui::actions::{
    CyclePlayMode, Escape, FocusFirst, FocusLast, FocusNextControl, FocusPrevControl, FocusSearch,
    NextTrack, PlayFocused, PrevTrack, RemoveFocused, SeekBackward, SeekBackwardLong, SeekForward,
    SeekForwardLong, SelectNext, SelectPrev, ToggleLyrics, ToggleMute, TogglePlay, ToggleSettings,
    ToggleShuffle, VolumeDown, VolumeUp,
};
use lyre_ui::platform::*;
use lyre_ui::shortcuts::CONTEXT;
use lyre_ui::shortcuts::keys;

/// 用「浏览」上下文：会被文字输入抢走的键。
const BROWSE: Option<&str> = Some("Lyre && !Input");
/// 用「全局」上下文：和文字输入不冲突的键。
const GLOBAL: Option<&str> = Some(CONTEXT);

/// 键位字符串一律取自 `lyre_ui::shortcuts::keys`，与设置页的表对应。
pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(keys::NEXT, NextTrack, GLOBAL),
        KeyBinding::new(keys::PREV, PrevTrack, GLOBAL),
        KeyBinding::new(keys::FOCUS_SEARCH, FocusSearch, GLOBAL),
        // Ctrl+, 是设置页的通用键位
        KeyBinding::new(keys::TOGGLE_SETTINGS, ToggleSettings, GLOBAL),
        // Esc 不放 GLOBAL
        KeyBinding::new(keys::ESCAPE, Escape, BROWSE),
        KeyBinding::new(keys::TOGGLE_PLAY, TogglePlay, BROWSE),
        KeyBinding::new(keys::SEEK_FORWARD, SeekForward, BROWSE),
        KeyBinding::new(keys::SEEK_BACKWARD, SeekBackward, BROWSE),
        KeyBinding::new(keys::SEEK_FORWARD_LONG, SeekForwardLong, BROWSE),
        KeyBinding::new(keys::SEEK_BACKWARD_LONG, SeekBackwardLong, BROWSE),
        // 只移动光标，不自动播放
        KeyBinding::new(keys::CURSOR_UP, SelectPrev, BROWSE),
        KeyBinding::new(keys::CURSOR_DOWN, SelectNext, BROWSE),
        KeyBinding::new(keys::CURSOR_FIRST, FocusFirst, BROWSE),
        KeyBinding::new(keys::CURSOR_LAST, FocusLast, BROWSE),
        KeyBinding::new(keys::PLAY_CURSOR, PlayFocused, BROWSE),
        // 移除只绑定 delete，backspace 见下面的 macOS 分支
        KeyBinding::new(keys::REMOVE_CURSOR, RemoveFocused, BROWSE),
        // 用 Ctrl 而不是 Alt 是因为 Alt 在 Windows 上会激活菜单栏
        KeyBinding::new(keys::VOLUME_UP, VolumeUp, BROWSE),
        KeyBinding::new(keys::VOLUME_DOWN, VolumeDown, BROWSE),
        KeyBinding::new(keys::MUTE, ToggleMute, BROWSE),
        KeyBinding::new(keys::LYRICS, ToggleLyrics, BROWSE),
        KeyBinding::new(keys::PLAY_MODE, CyclePlayMode, BROWSE),
        KeyBinding::new(keys::SHUFFLE, ToggleShuffle, BROWSE),
        // `/` 是搜索框的通用快捷键
        KeyBinding::new(keys::FOCUS_SEARCH_SLASH, FocusSearch, BROWSE),
        // Tab 走控件序列。绑 BROWSE 而不是 GLOBAL
        KeyBinding::new(keys::TAB_NEXT, FocusNextControl, BROWSE),
        KeyBinding::new(keys::TAB_PREV, FocusPrevControl, BROWSE),
    ]);

    // macOS 键盘那个位置就是 Backspace，不补这条 Mac 用户按不到移除
    #[cfg(target_os = "macos")]
    cx.bind_keys([KeyBinding::new("backspace", RemoveFocused, BROWSE)]);
}

#[cfg(test)]
mod tests {
    use lyre_ui::shortcuts::CONTEXT;

    /// 断言两个上下文不同，防止有人把它们合并回一个。
    #[test]
    fn browse_context_excludes_text_input_but_global_does_not() {
        use super::{BROWSE, GLOBAL};
        assert_ne!(BROWSE, GLOBAL);
        assert!(
            BROWSE.unwrap().contains("!Input"),
            "浏览上下文必须排除输入框，否则打字会触发全局快捷键"
        );
        assert!(
            !GLOBAL.unwrap().contains("!Input"),
            "全局上下文不该排除输入框 —— Alt+方向键、Ctrl+K 在输入时也要能用"
        );
    }

    /// 上下文串与 `RootView` 的 `key_context(..)` 必须同源。
    /// 对不上不会报错，只会「所有快捷键没反应」—— 所以这里显式钉住。
    #[test]
    fn both_contexts_are_built_on_the_shared_name() {
        use super::{BROWSE, GLOBAL};
        assert_eq!(GLOBAL, Some(CONTEXT), "全局上下文必须就是 CONTEXT 本身");
        assert!(
            BROWSE.is_some_and(|ctx| ctx.starts_with(CONTEXT)),
            "浏览上下文必须建立在 CONTEXT 之上：{BROWSE:?}"
        );
    }
}
