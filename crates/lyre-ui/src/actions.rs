//! 键盘动作：`KeyBinding` 定「哪个键」，元素树上的 `on_action` 定「谁响应」。
//! `↑`/`↓` 归列表光标且只移动、不自动播放；音量在 `Ctrl + ↑/↓`。

use crate::platform::*;

actions!(
    lyre,
    [
        TogglePlay,
        NextTrack,
        PrevTrack,
        SeekForward,
        SeekBackward,
        /// 快进 30 秒（`Shift + →`）
        SeekForwardLong,
        /// 快退 30 秒（`Shift + ←`）
        SeekBackwardLong,
        VolumeUp,
        VolumeDown,
        ToggleMute,
        /// 播放模式四态循环：顺序 → 随机 → 列表循环 → 单曲循环。
        CyclePlayMode,
        /// 列表光标上移一格（不播放）
        SelectPrev,
        /// 列表光标下移一格（不播放）
        SelectNext,
        FocusFirst,
        FocusLast,
        /// 播放光标所在曲目
        PlayFocused,
        /// 从曲库移除光标所在曲目（可撤销）
        RemoveFocused,
        /// 聚焦搜索框（`/` 或 `Ctrl+K`）
        FocusSearch,
        /// 随机播放二态直达：非随机时按下进随机，已是随机则回到顺序。
        ToggleShuffle,
        /// 一次只退一步，撤销的是上一步而不是重置界面。
        Escape,
        /// 打开 / 关闭设置面板
        ToggleSettings,
        /// 键盘焦点移到下一个控件（Tab）。gpui 没有默认的 Tab 绑定：
        /// `tab_index` 只把元素登记进 tab 序列，谁按 Tab 往前走由应用自己绑。
        FocusNextControl,
        /// 键盘焦点移到上一个控件（Shift + Tab）。
        FocusPrevControl,
        /// 打开 / 关闭歌词全屏视图，底栏按钮与 `L` 键共用这一个动作。
        ToggleLyrics,
    ]
);
