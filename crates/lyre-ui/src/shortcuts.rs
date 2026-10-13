//! 键位的唯一来源：`lyre-app/src/keymap.rs` 的绑定与设置页的展示表都读这里，两处必须一致。
//! 步进量也在这里 —— `RootView` 实际跳的量由这几个常量算，标签里的「5 秒」「30 秒」是手写的，靠测试钉住一致。

/// 一条快捷键的展示信息。`keys` 用 X11 键名的写法（`ctrl-up` 这类），与 `KeyBinding::new` 一致。
pub struct Shortcut {
    pub keys: &'static str,
    pub label: &'static str,
    /// 输入框聚焦时是否仍然生效。`false` 的条目绑在「浏览」上下文，打字时会被输入框抢走。
    pub while_typing: bool,
}

pub const SEEK_SECONDS: u64 = 5;
pub const SEEK_LONG_SECONDS: u64 = 30;
pub const VOLUME_STEP: f32 = 0.05;

/// 键位上下文名。`key_context(..)` 与 `keymap.rs` 的 `KeyBinding` 必须用同一个字符串，
/// 写错不报错，只是按键没反应。打字时让位给输入框的组合（`&& !Input`）见 `keymap.rs` 的 `BROWSE`。
pub const CONTEXT: &str = "Lyre";

/// 键位字符串，动作类型留在 `keymap.rs`。
pub mod keys {
    pub const TOGGLE_PLAY: &str = "space";
    pub const NEXT: &str = "alt-right";
    pub const PREV: &str = "alt-left";
    pub const SEEK_FORWARD: &str = "right";
    pub const SEEK_BACKWARD: &str = "left";
    pub const SEEK_FORWARD_LONG: &str = "shift-right";
    pub const SEEK_BACKWARD_LONG: &str = "shift-left";

    pub const FOCUS_SEARCH: &str = "ctrl-k";
    pub const FOCUS_SEARCH_SLASH: &str = "/";
    pub const TOGGLE_SETTINGS: &str = "ctrl-,";
    pub const ESCAPE: &str = "escape";

    pub const CURSOR_UP: &str = "up";
    pub const CURSOR_DOWN: &str = "down";
    pub const CURSOR_FIRST: &str = "home";
    pub const CURSOR_LAST: &str = "end";
    pub const PLAY_CURSOR: &str = "enter";
    pub const REMOVE_CURSOR: &str = "delete";

    pub const VOLUME_UP: &str = "ctrl-up";
    pub const VOLUME_DOWN: &str = "ctrl-down";
    pub const MUTE: &str = "m";
    pub const PLAY_MODE: &str = "s";
    pub const SHUFFLE: &str = "r";
    pub const LYRICS: &str = "l";
    /// Tab 走控件序列。gpui 不自带这条绑定，见 `actions::FocusNextControl`。
    pub const TAB_NEXT: &str = "tab";
    pub const TAB_PREV: &str = "shift-tab";
}

use keys::*;

pub const GROUP_PLAYBACK: &[Shortcut] = &[
    Shortcut {
        keys: TOGGLE_PLAY,
        label: "播放 / 暂停",
        while_typing: false,
    },
    Shortcut {
        keys: PLAY_CURSOR,
        label: "播放光标所在曲目",
        while_typing: false,
    },
    Shortcut {
        keys: NEXT,
        label: "下一曲",
        while_typing: true,
    },
    Shortcut {
        keys: PREV,
        label: "上一曲",
        while_typing: true,
    },
    Shortcut {
        keys: SEEK_FORWARD,
        label: "快进 5 秒",
        while_typing: false,
    },
    Shortcut {
        keys: SEEK_BACKWARD,
        label: "快退 5 秒",
        while_typing: false,
    },
    Shortcut {
        keys: SEEK_FORWARD_LONG,
        label: "快进 30 秒",
        while_typing: false,
    },
    Shortcut {
        keys: SEEK_BACKWARD_LONG,
        label: "快退 30 秒",
        while_typing: false,
    },
];

pub const GROUP_BROWSE: &[Shortcut] = &[
    Shortcut {
        keys: CURSOR_UP,
        label: "光标上移",
        while_typing: false,
    },
    Shortcut {
        keys: CURSOR_DOWN,
        label: "光标下移",
        while_typing: false,
    },
    Shortcut {
        keys: CURSOR_FIRST,
        label: "光标到第一行",
        while_typing: false,
    },
    Shortcut {
        keys: CURSOR_LAST,
        label: "光标到最后一行",
        while_typing: false,
    },
    Shortcut {
        keys: REMOVE_CURSOR,
        label: "从曲库移除光标曲目（可撤销）",
        while_typing: false,
    },
    Shortcut {
        keys: FOCUS_SEARCH,
        label: "聚焦搜索框",
        while_typing: true,
    },
    Shortcut {
        keys: FOCUS_SEARCH_SLASH,
        label: "聚焦搜索框",
        while_typing: false,
    },
    Shortcut {
        keys: TOGGLE_SETTINGS,
        label: "打开 / 关闭设置",
        while_typing: true,
    },
    Shortcut {
        keys: ESCAPE,
        label: "后退一层：歌词 → 返回 → 面板 → 搜索",
        while_typing: false,
    },
    Shortcut {
        keys: TAB_NEXT,
        label: "焦点移到下一个控件",
        while_typing: false,
    },
    Shortcut {
        keys: TAB_PREV,
        label: "焦点移到上一个控件",
        while_typing: false,
    },
];

pub const GROUP_SOUND: &[Shortcut] = &[
    Shortcut {
        keys: VOLUME_UP,
        label: "音量 +5%",
        while_typing: false,
    },
    Shortcut {
        keys: VOLUME_DOWN,
        label: "音量 −5%",
        while_typing: false,
    },
    Shortcut {
        keys: MUTE,
        label: "静音",
        while_typing: false,
    },
    Shortcut {
        keys: PLAY_MODE,
        label: "播放模式循环",
        while_typing: false,
    },
    Shortcut {
        keys: SHUFFLE,
        label: "随机播放开关",
        while_typing: false,
    },
    Shortcut {
        keys: LYRICS,
        label: "歌词全屏",
        while_typing: false,
    },
];

/// 设置页按这个顺序分组渲染。
pub const GROUPS: &[(&str, &[Shortcut])] = &[
    ("播放", GROUP_PLAYBACK),
    ("浏览", GROUP_BROWSE),
    ("音量与模式", GROUP_SOUND),
];

#[cfg(test)]
mod tests {
    use super::{SEEK_LONG_SECONDS, SEEK_SECONDS, VOLUME_STEP, keys};

    /// 每条键位都得出现在展示表里，否则设置页少一行、用户以为没这个键。
    #[test]
    fn every_bound_key_is_displayed() {
        let displayed = super::GROUPS
            .iter()
            .flat_map(|(_, rows)| rows.iter().map(|row| row.keys))
            .collect::<Vec<_>>();
        let bound = [
            keys::TOGGLE_PLAY,
            keys::NEXT,
            keys::PREV,
            keys::SEEK_FORWARD,
            keys::SEEK_BACKWARD,
            keys::SEEK_FORWARD_LONG,
            keys::SEEK_BACKWARD_LONG,
            keys::FOCUS_SEARCH,
            keys::FOCUS_SEARCH_SLASH,
            keys::TOGGLE_SETTINGS,
            keys::ESCAPE,
            keys::CURSOR_UP,
            keys::CURSOR_DOWN,
            keys::CURSOR_FIRST,
            keys::CURSOR_LAST,
            keys::PLAY_CURSOR,
            keys::REMOVE_CURSOR,
            keys::VOLUME_UP,
            keys::VOLUME_DOWN,
            keys::MUTE,
            keys::PLAY_MODE,
            keys::SHUFFLE,
            keys::LYRICS,
            keys::TAB_NEXT,
            keys::TAB_PREV,
        ];
        assert_eq!(
            bound.len(),
            displayed.len(),
            "绑定了 {} 条键位、展示了 {} 条",
            bound.len(),
            displayed.len()
        );
        for key in bound {
            assert!(displayed.contains(&key), "`{key}` 绑定了却没进展示表");
        }
    }

    #[test]
    fn key_strings_are_unique_per_label() {
        // 同一个键可以出现在两处（`/` 与 `Ctrl+K` 都聚焦搜索），但同一组里不能重复。
        for (name, rows) in super::GROUPS {
            for (index, row) in rows.iter().enumerate() {
                assert!(
                    !rows[..index].iter().any(|earlier| earlier.keys == row.keys),
                    "{name} 组里 `{}` 出现了两次",
                    row.keys
                );
            }
        }
    }

    #[test]
    fn step_labels_match_the_table() {
        // 文案里的数字是手写的，改步进量时这条会红，提醒同步 `GROUP_PLAYBACK` / `GROUP_SOUND`。
        assert_eq!(SEEK_SECONDS, 5);
        assert_eq!(SEEK_LONG_SECONDS, 30);
        assert_eq!((VOLUME_STEP * 100.0).round(), 5.0);
    }
}
