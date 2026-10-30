//! 悬停提示。gpui 的 `Div::tooltip` 要的是返回 `AnyView` 的闭包，所以提示必须是视图实体。

use crate::platform::*;
use crate::theme;

pub struct TextTooltip {
    label: SharedString,
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(theme::GAP_SM))
            .py(px(theme::GAP_XS))
            .rounded(px(theme::RADIUS_SM))
            .bg(theme::elevated())
            .border_1()
            .border_color(theme::border())
            .text_size(px(theme::FONT_CAPTION))
            .text_color(theme::text())
            .child(self.label.clone())
    }
}

pub fn builder(
    label: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let label = label.into();
    move |_window: &mut Window, cx: &mut App| {
        let label = label.clone();
        cx.new(|_| TextTooltip { label }).into()
    }
}
