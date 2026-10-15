//! 封面槽的共用取图流程：查缓存 → 未命中且并非「确定无封面」才发请求 → 否则由调用方画占位。

use std::sync::Arc;

use lyre_core::TrackId;

use crate::cover::CoverCache;
use crate::entities::LibraryEntity;
use crate::icon::{Glyph, icon_inherit};
use crate::platform::*;
use crate::theme;

/// 无封面占位 / 分类块的稳定取色：按首字符在调色板上取值。
pub fn monogram_color(initial: char) -> Rgba {
    const PALETTE: [(u8, u8, u8); 6] = [
        (0x6d, 0x5b, 0x9c),
        (0x4a, 0x6c, 0x8c),
        (0x8c, 0x5a, 0x5a),
        (0x5a, 0x7a, 0x6a),
        (0x7a, 0x6a, 0x8c),
        (0x8c, 0x7a, 0x5a),
    ];
    let (r, g, b) = PALETTE[u32::from(initial) as usize % PALETTE.len()];
    rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
}

/// 封面槽的三态。必须区分「排队中」与「没有封面」：
/// 前者画骨架屏（内容在路上），后者才是真占位（首字色块 / 人形图标 / 音符）。
pub enum Cover {
    Ready(Arc<RenderImage>),
    Missing,
    Pending,
}

/// 取封面。未命中且并非「确定无封面」时发起一次解码请求，返回 [`Cover::Pending`]。
/// 分组场景要传该组的 `representative`（第一个有封面的曲目）而非第一首。
/// `size` 是这一格的逻辑像素边长：缓存按它分档取图。
pub fn cover_state(
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    id: TrackId,
    size: f32,
    cx: &mut App,
) -> Cover {
    let cached = covers.read(cx).ready(id, size);
    if let Some(image) = cached {
        return Cover::Ready(image);
    }
    // 漏掉这一档，无封面的曲目会每帧重发一次解码请求
    if covers.read(cx).is_missing(id, size) {
        return Cover::Missing;
    }

    // 每个可见行都要走一次，所以必须是哈希查表而不是线性扫
    let path = App::update_entity(cx, library, |library, _| {
        library.playlist().by_id(id).map(|track| track.path.clone())
    });
    match path {
        Some(path) => {
            covers.update(cx, |covers, cx| covers.request(id, path, size, cx));
            Cover::Pending
        }
        // 曲库里查不到这首：没有可解码的来源，直接判缺失
        None => Cover::Missing,
    }
}

/// 只要位图：`Ready` 给图，其余一律 `None`。
pub fn cover_image(
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    id: TrackId,
    size: f32,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    match cover_state(covers, library, id, size, cx) {
        Cover::Ready(image) => Some(image),
        Cover::Missing | Cover::Pending => None,
    }
}

/// 解码期间的骨架块：暗灰对角渐变。宽度交给父容器（`w_full`），高度由调用方给。
/// 不做动画：一屏常有几十张同时排队，逐帧 shimmer 会让这些槽位每帧重绘。
pub fn skeleton(height: f32, radius: f32) -> Div {
    div()
        .w_full()
        .h(px(height))
        .rounded(px(radius))
        .bg(linear_gradient(
            135.0,
            linear_color_stop(theme::surface(), 0.0),
            linear_color_stop(theme::elevated(), 1.0),
        ))
}

/// 歌单封面的一格：用户指定的图 → 成员的内嵌封面 → 按名字派生的渐变块。
#[allow(clippy::too_many_arguments)]
pub fn list_cover(
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    size: f32,
    radius: f32,
    cover_file: Option<&std::path::Path>,
    cover_id: Option<TrackId>,
    seed: &str,
    cx: &mut App,
) -> AnyElement {
    if let Some(path) = cover_file {
        let owned = path.to_path_buf();
        covers.update(cx, |cache, cx| cache.request_file(owned, size, cx));
        return match covers.read(cx).ready_file(path, size) {
            Some(image) => image_cell(image, size, radius),
            // 还在解码、或那张图根本读不出来：退到派生块
            None => derived_cover(size, radius, seed),
        };
    }

    let Some(id) = cover_id else {
        // 空歌单没有任何曲目能代表封面
        return derived_cover(size, radius, seed);
    };
    match cover_state(covers, library, id, size, cx) {
        Cover::Ready(image) => image_cell(image, size, radius),
        Cover::Missing => derived_cover(size, radius, seed),
        Cover::Pending => div()
            .flex_none()
            .size(px(size))
            .rounded(px(radius))
            .child(skeleton(size, radius))
            .into_any_element(),
    }
}

/// 列表 / 侧栏的行首缩略图，尺寸、圆角与缺封面时的占位图标由参数给。
pub fn thumb(
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    id: TrackId,
    size: f32,
    radius: f32,
    placeholder: Glyph,
    cx: &mut App,
) -> AnyElement {
    let state = cover_state(covers, library, id, size, cx);

    let cell = div()
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius))
        .bg(theme::elevated());

    match state {
        Cover::Ready(image) => cell
            .child(
                img(ImageSource::Render(image))
                    .size_full()
                    .rounded(px(radius))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element(),
        Cover::Missing => cell
            .border_1()
            .border_color(theme::border())
            .text_color(theme::text_faint())
            .child(icon_inherit(placeholder, 16.))
            .into_any_element(),
        Cover::Pending => cell.child(skeleton(size, radius)).into_any_element(),
    }
}

/// 一张已解码的封面铺满那一格。
pub fn image_cell(image: Arc<RenderImage>, size: f32, radius: f32) -> AnyElement {
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(radius))
        .overflow_hidden()
        .child(
            img(ImageSource::Render(image))
                .size_full()
                .rounded(px(radius))
                .object_fit(ObjectFit::Cover),
        )
        .into_any_element()
}

/// 按名字派生的渐变块。
fn derived_cover(size: f32, radius: f32, seed: &str) -> AnyElement {
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(radius))
        .border_1()
        .border_color(theme::border())
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .bg(linear_gradient(
            135.0,
            linear_color_stop(theme::hero_tint(Some(seed)).opacity(0.55), 0.0),
            linear_color_stop(theme::elevated(), 1.0),
        ))
        .child(crate::icon::icon(
            crate::icon::Glyph::Music,
            (size * 0.25).round(),
            theme::text_faint(),
        ))
        .into_any_element()
}
