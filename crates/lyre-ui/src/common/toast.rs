//! 撤销提示（toast）：破坏性操作立即生效、不弹确认框，用底部提示提供限时撤销窗口。
//! 同一时刻只显示一条，新的顶掉旧的；超时后撤销不可用。

use std::time::Duration;

use crate::platform::*;
use crate::theme;

pub const TOAST_DURATION: Duration = Duration::from_secs(6);

struct Pending {
    message: SharedString,
    action_label: &'static str,
    action: Option<UndoAction>,
}

type UndoAction = Box<dyn FnOnce(&mut Window, &mut App) + 'static>;

pub struct ToastEntity {
    pending: Option<Pending>,
    /// 每次 show 递增；定时器只在本代仍是当前代时才关闭，否则前一次的定时器会提前关掉新提示。
    generation: u64,
    _expiry: Option<Task<()>>,
}

impl ToastEntity {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let _ = cx;
        Self {
            pending: None,
            generation: 0,
            _expiry: None,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.pending.is_some()
    }

    /// 显示一条带操作按钮的提示。
    // `_expiry` 只写不读，作用是保活：drop 时取消上一个定时器。
    #[allow(clippy::used_underscore_binding)]
    pub fn show(
        &mut self,
        message: impl Into<SharedString>,
        action_label: &'static str,
        action: impl FnOnce(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;

        self.pending = Some(Pending {
            message: message.into(),
            action_label,
            action: Some(Box::new(action)),
        });
        cx.notify();

        let executor = cx.background_executor().clone();
        self._expiry = Some(cx.spawn(async move |this, cx| {
            executor.timer(TOAST_DURATION).await;
            this.update(cx, |toast, cx| {
                if toast.generation == generation {
                    toast.dismiss(cx);
                }
            })
            .ok();
        }));
    }

    pub fn show_undo(
        &mut self,
        message: impl Into<SharedString>,
        undo: impl FnOnce(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.show(message, "撤销", undo, cx);
    }

    /// 显示一条没有操作按钮的提示（错误、状态告知）。
    pub fn show_plain(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.show(message, "", |_, _| {}, cx);
    }

    // 置 `None` 即取消定时器：`Task` 一 drop 就中止。
    #[allow(clippy::used_underscore_binding)]
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.pending.take().is_some() {
            self._expiry = None;
            cx.notify();
        }
    }
}

impl Render for ToastEntity {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 无内容时返回零尺寸占位；两条分支都要转 `AnyElement`，否则 `impl IntoElement` 的具体类型对不上
        let Some(pending) = self.pending.as_mut() else {
            return div().into_any_element();
        };

        let message = pending.message.clone();
        let action_label = pending.action_label;
        let has_action = pending.action.is_some() && !action_label.is_empty();

        let action_button = has_action.then(|| {
            let entity = cx.entity().downgrade();
            div()
                .id("toast-action")
                .flex_none()
                .px(px(theme::GAP_SM))
                .py(px(2.0))
                .rounded(px(theme::RADIUS_SM))
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text())
                .cursor_pointer()
                // 撤销提示只活 6 秒，键盘用户也得够得着，所以它进 Tab 序列。
                .tab_index(crate::common::button::TAB_STOP)
                .focus_visible(theme::focus_ring)
                .role(Role::Button)
                .aria_label("撤销")
                .hover(|style| style.bg(theme::hover()))
                .on_click(move |_, window, cx| {
                    entity
                        .update(cx, |toast, cx| {
                            if let Some(pending) = toast.pending.as_mut()
                                && let Some(action) = pending.action.take()
                            {
                                // 先执行撤销，再关闭提示：撤销本身可能又要弹新提示
                                action(window, cx);
                            }
                            toast.dismiss(cx);
                        })
                        .ok();
                })
                .child(action_label)
        });

        div()
            .id("toast")
            .flex()
            .items_center()
            .gap(px(theme::GAP_MD))
            .px(px(theme::GAP_LG))
            .py(px(theme::GAP_MD))
            .rounded(px(theme::RADIUS_LG))
            .bg(theme::elevated())
            .border_1()
            .border_color(theme::border())
            .text_size(px(theme::FONT_CAPTION))
            .text_color(theme::text())
            .child(div().max_w(px(420.)).truncate().child(message))
            .children(action_button)
            .into_any_element()
    }
}
