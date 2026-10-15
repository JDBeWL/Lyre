//! 自建歌单的右键菜单与重命名 / 删除对话框。
//! 输入框必须在开窗前建：它的构建器会被反复调用。

use lyre_core::CollectionId;

use crate::common::button::{ButtonKind, text_button};
use crate::entities::LibraryEntity;
use crate::platform::*;
use crate::theme;

/// 自建歌单的右键菜单。
pub(crate) fn collection_menu(
    library: Entity<LibraryEntity>,
    id: CollectionId,
    name: String,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _window, _cx| {
        let rename_library = library.clone();
        let rename_name = name.clone();
        let delete_library = library.clone();
        let delete_name = name.clone();
        menu.item(PopupMenuItem::new("重命名").on_click(move |_, window, cx| {
            open_rename_dialog(rename_library.clone(), id, &rename_name, window, cx);
        }))
        .item(
            PopupMenuItem::new("删除歌单").on_click(move |_, window, cx| {
                open_delete_dialog(delete_library.clone(), id, delete_name.clone(), window, cx);
            }),
        )
    }
}

/// 组件库的 `Dialog` 不自己画按钮（`footer` 默认 `None`），所以底部用项目自己的 `text_button`。
fn dialog_footer(
    cancel_id: &'static str,
    ok_id: &'static str,
    on_ok: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(theme::GAP_SM))
        .pt(px(theme::GAP_MD))
        .child(text_button(
            cancel_id,
            "取消",
            ButtonKind::Ghost,
            |_, window, cx| {
                window.close_dialog(cx);
            },
        ))
        .child(text_button(ok_id, "确定", ButtonKind::Primary, on_ok))
}

/// 提交重命名。返回 `false` 表示名字无效，对话框不该关。
fn commit_rename(
    library: &Entity<LibraryEntity>,
    id: CollectionId,
    input: &Entity<InputState>,
    cx: &mut App,
) -> bool {
    let name = input.read(cx).value().trim().to_owned();
    if name.is_empty() {
        return false;
    }
    library.update(cx, |library, cx| library.rename_collection(id, &name, cx));
    true
}

/// 重命名对话框。输入框在开窗之前建：内容构建器会被反复调用，
/// 在里面建实体等于每重绘一次就把用户正在打的字清空。
fn open_rename_dialog(
    library: Entity<LibraryEntity>,
    id: CollectionId,
    current: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
    // 打开即聚焦并全选：目的是改名，不是往旧名后面追加。
    input.update(cx, |state, cx| {
        state.select_all(window, cx);
        state.focus(window, cx);
    });
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let enter_library = library.clone();
        let enter_input = input.clone();
        let ok_library = library.clone();
        let ok_input = input.clone();
        dialog
            .title("重命名歌单")
            .child(input.clone())
            .footer(dialog_footer(
                "rename-cancel",
                "rename-ok",
                move |_, window, cx| {
                    if commit_rename(&ok_library, id, &ok_input, cx) {
                        window.close_dialog(cx);
                    }
                },
            ))
            // 回车走这条：与「确定」同一函数，只是关窗由对话框自己按返回值决定
            .on_ok(move |_, _window, cx| commit_rename(&enter_library, id, &enter_input, cx))
    });
}

/// 删除确认对话框。删除立即生效，不提供撤销。
fn open_delete_dialog(
    library: Entity<LibraryEntity>,
    id: CollectionId,
    name: String,
    window: &mut Window,
    cx: &mut App,
) {
    // 数字在开窗前算好：构建器会被反复调用
    let count = library.read(cx).collection_len(id);
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let library = library.clone();
        dialog
            .title("删除歌单")
            .child(format!(
                "确定删除「{name}」？{count} 首曲目仍会留在曲库里。"
            ))
            .on_ok(move |_, _, cx| {
                library.update(cx, |library, cx| library.delete_collection(id, cx));
                true
            })
    });
}
