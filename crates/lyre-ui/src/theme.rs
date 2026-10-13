//! 设计令牌。
//! 所有文字色对背景的对比度必须 ≥ 4.5:1（WCAG AA），由本文件末尾的测试强制。

use crate::platform::*;

/// 窗口底层：卡片之间的缝（gutter）与窗口四周的留白，纯黑。
pub fn window_base() -> Rgba {
    rgb(0x000000)
}
/// 主内容区，也是所有卡片（侧栏卡片、主区卡片、右面板卡片）的底色。
pub fn bg() -> Rgba {
    rgb(0x121212)
}
/// 顶栏 / 底栏这些通栏窗口外壳的背景，与缝同色。
pub fn chrome() -> Rgba {
    rgb(0x000000)
}

/// 侧边栏 / 右面板卡片的背景，与 [`bg`] 刻意同色：分区靠缝，测试钉住两者相等。
pub fn panel() -> Rgba {
    rgb(0x121212)
}

/// 主区顶部渐变带的起始色，由一个稳定字符串派生出固定色相。
/// 不从封面取主色：解码是异步的，首帧会闪，且取色结果不稳定。
pub fn hero_tint(seed: Option<&str>) -> Rgba {
    const TINTS: [(u8, u8, u8); 6] = [
        (0x53, 0x45, 0x6b), // 灰紫
        (0x3d, 0x55, 0x66), // 灰蓝
        (0x6b, 0x4a, 0x45), // 灰砖
        (0x44, 0x5c, 0x4c), // 灰绿
        (0x5c, 0x4c, 0x66), // 灰藕
        (0x63, 0x56, 0x3f), // 灰棕
    ];

    // 兜底色要比 `bg` 亮才看得出渐变，明度对齐 `TINTS` 才不会在有/无种子之间跳。
    let Some(seed) = seed else {
        return rgb(0x3a_3f_4a);
    };

    // 手写 FNV-1a：`DefaultHasher` 不承诺跨版本稳定，而同一张专辑的颜色必须每次都一样。
    let mut hash: u32 = 2166136261;
    for byte in seed.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16777619);
    }
    let (r, g, b) = TINTS[hash as usize % TINTS.len()];
    rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
}
/// 搜索框、卡片、输入框的容器底色。
pub fn surface() -> Rgba {
    rgb(0x181818)
}
/// 浮在面板之上的一档（按钮悬停底、列头）。
pub fn elevated() -> Rgba {
    rgb(0x282828)
}
/// 列表行的悬停底色，比 [`elevated`] 暗一档。
pub fn hover() -> Rgba {
    rgb(0x1a1a1a)
}
/// 键盘光标所在行的底色，比 [`elevated`] 亮一档。
/// 必须与 [`hover`] 分两个令牌：`hover` 对 [`bg`] 只有 1.08:1，同色时分不清「指针压着」与「光标在这」。
pub fn cursor() -> Rgba {
    rgb(0x2a2a2a)
}
pub fn border() -> Rgba {
    rgb(0x333333)
}
/// 比 [`border`] 更弱的分隔线，用于列表内部分隔。
pub fn border_soft() -> Rgba {
    rgb(0x1c1c1c)
}

pub fn text() -> Rgba {
    rgb(0xffffff)
}
/// 歌手、专辑、时长这类次要信息。
pub fn text_muted() -> Rgba {
    rgb(0xb3b3b3)
}
/// 序号、占位符、次要图标。再暗一档（如 `#6a6a6a`）在 `bg` 上就到不了 AA。
pub fn text_faint() -> Rgba {
    rgb(0xa7a7a7)
}

/// 品牌绿：正在播的行、行首竖条、无损徽章、爱心。控件的填充走 [`control`]。
pub fn accent() -> Rgba {
    rgb(0x1db954)
}
pub fn accent_hover() -> Rgba {
    rgb(0x1ed760)
}
/// 绿色文字：正在播的曲名、徽章。
pub fn accent_text() -> Rgba {
    rgb(0x1ed760)
}
/// 选中行 / 拖放高亮的淡底色：`accent` 的 0.15 alpha，半透明。
pub fn accent_soft() -> Rgba {
    rgb(0x1db954).opacity(0.15)
}

/// 主控件的填充色：播放键、进度条与音量条的已播段、输入框光标。
/// 与 [`fn@text`] 同值但语义不同 —— 那是文字色，这一档是「能按、在动」的东西。
pub fn control() -> Rgba {
    rgb(0xffffff)
}
/// [`control`] 的悬停档。压暗一档而不是提亮：白已经是上限。
pub fn control_hover() -> Rgba {
    rgb(0xdcdcdc)
}
/// 压在 [`control`] 上的图形（播放三角、勾选框的对勾）。
pub fn text_on_control() -> Rgba {
    chrome()
}

pub fn danger() -> Rgba {
    rgb(0xe5484d)
}
/// 红底上的文字色（窗口关闭键的叉）。必须黑字：白压 `#e5484d` 达不到 AA。
pub fn text_on_danger() -> Rgba {
    rgb(0x000000)
}
pub fn danger_soft() -> Rgba {
    rgb(0xe5484d).opacity(0.12)
}
/// 扫描完成、就绪等正向指示，与 [`accent`] 同值。
pub fn success() -> Rgba {
    rgb(0x1db954)
}

/// 进度 / 音量轨道的底色，要和已播段那一档白拉开对比。
pub fn track() -> Rgba {
    rgb(0x555555)
}

/// 格式标签（wav / mp3）的底色。
pub fn tag_bg() -> Rgba {
    rgb(0x2a2a2a)
}

/// 完全透明的背景，用于「未选中行」占位。
pub fn transparent() -> Rgba {
    rgba(0x00000000)
}

// 字重从 600 起：500 在深色底上几乎看不出粗细变化。

/// 徽标、快捷键提示
pub const FONT_MICRO: f32 = 11.0;
/// 时长、时间戳、辅助信息、格式标签的基准
pub const FONT_CAPTION: f32 = 12.0;
/// 列表副标题、按钮、导航项
pub const FONT_BODY: f32 = 14.0;
/// 列表主标题（歌名）
pub const FONT_TRACK: f32 = 15.0;
/// 面板标题、底栏当前曲名
pub const FONT_SUBTITLE: f32 = 16.0;
/// 页面大标题（曲库总览）
pub const FONT_TITLE: f32 = 24.0;
/// 页面头部标题（压在渐变色带上的那一行），比 `FONT_TITLE` 大一档。
pub const FONT_HERO: f32 = 32.0;
/// 字号之间的相对关系是布局前提，写成编译期断言：`FONT_HERO` 合并进 `FONT_TITLE` 会让页头矮进色带，
/// 超过 40px 会压过渐变色带。
const _: () = assert!(FONT_HERO > FONT_TITLE);
const _: () = assert!(FONT_HERO > FONT_SUBTITLE);
const _: () = assert!(FONT_HERO <= 40.0);
/// 空状态
pub const FONT_DISPLAY: f32 = 30.0;
/// 格式标签（wav / mp3）的字号
pub const FONT_TAG: f32 = 10.0;

pub const WEIGHT_TITLE: FontWeight = FontWeight::BOLD;
pub const WEIGHT_SUBTITLE: FontWeight = FontWeight::SEMIBOLD;
/// 按钮与导航项的字重，600。
pub const WEIGHT_BUTTON: FontWeight = FontWeight::SEMIBOLD;
/// 列表歌名：常态 regular，播放态会额外加粗（见 `track_list`）。
pub const WEIGHT_TRACK: FontWeight = FontWeight::NORMAL;

pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 6.0;
pub const RADIUS_LG: f32 = 8.0;
pub const RADIUS_XL: f32 = 12.0;
/// 窗口级卡片（侧栏卡片、主区卡片）的圆角。
pub const RADIUS_CARD: f32 = 8.0;
/// 卡片之间的缝宽，也是卡片到窗口边缘的留白。
pub const GUTTER: f32 = 8.0;
/// 胶囊圆角。gpui 里给一个远大于高度的数就等价于全圆。
pub const RADIUS_PILL: f32 = 999.0;

pub const GAP_XS: f32 = 4.0;
pub const GAP_SM: f32 = 8.0;
pub const GAP_MD: f32 = 12.0;
pub const GAP_LG: f32 = 16.0;
pub const GAP_XL: f32 = 24.0;

/// 顶部栏高度：32px 的按钮 + 上下各 8px（窗口控件同尺寸，见 `window_controls`）。
pub const TOP_BAR_H: f32 = 48.0;
pub const BOTTOM_BAR_H: f32 = 80.0;
/// 底部控制栏在窄窗口下的高度（两行：传输区 + 进度条）。多出的 24px = 进度条行 22px + 一档间距。
/// 切换阈值见 `shell::STACKED_BOTTOM_MIN`。
pub const BOTTOM_BAR_H_STACKED: f32 = 104.0;

/// 底栏高度。底栏自己和根视图的 toast 定位都要读它，两处各算一遍会让 toast 压住底栏。
pub fn bottom_bar_height(stacked: bool) -> f32 {
    if stacked {
        BOTTOM_BAR_H_STACKED
    } else {
        BOTTOM_BAR_H
    }
}

/// 侧边导航展开宽度（里面要放圆形封面 + 两行文字）。
pub const SIDEBAR_W: f32 = 232.0;
pub const SIDEBAR_ICON_W: f32 = 64.0;
pub const SIDEBAR_ITEM_H: f32 = 40.0;
pub const SIDEBAR_INDICATOR_W: f32 = 3.0;
pub const RIGHT_PANEL_W: f32 = 320.0;
/// 底栏左栏宽度（曲目身份）。
pub const BOTTOM_LEFT_W: f32 = 240.0;
/// 底栏右栏宽度（功能入口 + 音量）。
/// 比 [`BOTTOM_LEFT_W`] 宽：那一排入口在 240px 里放不下。
pub const BOTTOM_RIGHT_W: f32 = 320.0;
pub const BOTTOM_CENTER_MAX: f32 = 620.0;
pub const ROW_H_COMPACT: f32 = 40.0;
pub const ROW_H_COMFORTABLE: f32 = 52.0;
/// 行首「光标在这」竖条的宽度。绝对定位，不占布局 —— 做成 flex 子元素会把整表列推歪。
pub const ROW_CURSOR_BAR_W: f32 = 3.0;
pub const COLUMN_HEADER_H: f32 = 36.0;
pub const SEARCH_H: f32 = 36.0;
pub const SEARCH_W: f32 = 280.0;
pub const MINI_COVER: f32 = 56.0;

/// 键盘焦点环的宽度。画在元素外侧，不占布局。
pub const FOCUS_RING_W: f32 = 2.0;

/// 键盘焦点环，给 gpui 的 `.focus_visible()` 用（只在上一次输入是键盘时应用，鼠标点过的不留圈）。
/// 用 `box_shadow` 而不是 `border`：gpui 的盒子高度含 border，聚焦那一帧会把元素撑大一圈。
/// 颜色写成 [`accent_text`] 的 HSL 形式：gpui 的 `BoxShadow` 只吃 `Hsla`，没有 `Rgba → Hsla` 的转换。
fn focus_ring_color() -> Hsla {
    // gpui 的 `Hsla.h` 是 0–1 的圈数而不是角度（`From<Hsla> for Rgba` 里直接拿 `h * 6.0` 取区间），
    // 写成 141.4 会被当成绕了 141 圈、落在别的色相上。
    hsla(0.3927928, 0.755102, 0.480392, 1.0)
}

pub fn focus_ring(style: StyleRefinement) -> StyleRefinement {
    style.shadow(vec![
        BoxShadow::new(px(0.), px(0.), focus_ring_color()).spread_radius(px(FOCUS_RING_W)),
    ])
}

/// 卡片 / 主按钮悬停时的上浮阴影。gpui-pre 0.3.8 的普通元素没有 `transform`
/// （`scale` / `translate` 只作用于 `Path` / `Scene`），上浮只能靠 `box-shadow` + 底色提亮。
pub fn hover_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(6.), hsla(0., 0., 0., 0.45)).blur_radius(px(16.))]
}

/// WCAG 2.1 相对亮度。
#[cfg(test)]
fn relative_luminance(color: Rgba) -> f32 {
    fn channel(c: f32) -> f32 {
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// 把 `fg` 合成到 `bg` 上。`opacity()` 只改 alpha（非预乘），所以要按 source-over 算。
#[cfg(test)]
fn over(fg: Rgba, bg: Rgba) -> Rgba {
    Rgba {
        r: fg.r * fg.a + bg.r * (1.0 - fg.a),
        g: fg.g * fg.a + bg.g * (1.0 - fg.a),
        b: fg.b * fg.a + bg.b * (1.0 - fg.a),
        a: 1.0,
    }
}

/// WCAG 2.1 对比度，返回 1.0 ~ 21.0。两个参数都允许带透明度，先依次压到窗口底色上再算。
/// 这一步不能省：`accent_soft` 这类半透明标记色只有合成后才是屏幕上真正的颜色。
#[cfg(test)]
fn contrast_ratio(foreground: Rgba, background: Rgba) -> f32 {
    let base = bg();
    let bg_opaque = over(background, base);
    let fg_opaque = over(foreground, bg_opaque);

    let (l1, l2) = (relative_luminance(fg_opaque), relative_luminance(bg_opaque));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    // 不用 glob 导入：`platform::*` 里的 gpui `test` 宏会遮蔽内置 `#[test]`，导致无限递归（见 platform.rs）。
    use crate::platform::{Rgba, rgb};

    use super::contrast_ratio;
    use super::relative_luminance;

    /// WCAG AA 正文的门槛。
    const AA: f32 = 4.5;

    /// 断言这对前景 / 背景达到 AA。
    fn assert_pass(fg: Rgba, bg: Rgba, name: &str) {
        let ratio = contrast_ratio(fg, bg);
        assert!(
            ratio >= AA,
            "{name} 的对比度只有 {ratio:.2}:1，低于 AA 要求的 {AA}:1"
        );
    }

    #[test]
    fn text_colors_pass_aa_on_every_background_layer() {
        // chrome 也在被测层里：顶栏 / 底栏全屏常驻、文字最密。
        let layers = [
            ("bg", super::bg()),
            ("panel", super::panel()),
            ("elevated", super::elevated()),
            ("surface", super::surface()),
            ("chrome", super::chrome()),
        ];
        let colors = [
            ("text", super::text()),
            ("text_muted", super::text_muted()),
            ("text_faint", super::text_faint()),
        ];

        for (layer_name, layer) in layers {
            for (color_name, color) in colors {
                assert_pass(color, layer, &format!("{color_name} on {layer_name}"));
            }
        }
    }

    #[test]
    fn accent_variants_pass_aa_in_their_intended_role() {
        assert_pass(super::accent_text(), super::bg(), "accent_text on bg");
        assert_pass(super::accent_text(), super::panel(), "accent_text on panel");
        assert_pass(super::accent(), super::panel(), "accent on panel");
    }

    #[test]
    fn control_fill_keeps_black_glyphs_legible() {
        assert_pass(super::control(), super::bg(), "control on bg");
        assert_pass(
            super::text_on_control(),
            super::control(),
            "glyph on control",
        );
        assert_pass(
            super::text_on_control(),
            super::control_hover(),
            "glyph on control_hover",
        );

        // 反向锁定：控件上的图形必须是黑的，白压白看不见。
        let white_on_control = contrast_ratio(super::text(), super::control());
        assert!(
            white_on_control < AA,
            "白字压在 control 上只有 {white_on_control:.2}:1 —— 实心控件必须用 text_on_control"
        );
    }

    #[test]
    fn window_close_button_uses_black_glyph_on_red() {
        // 窗口关闭键悬停时是红底 + 叉，Windows 原生样式用白叉；白压红达不到 AA，符号必须用 text_on_danger。
        assert_pass(
            super::text_on_danger(),
            super::danger(),
            "text_on_danger on danger",
        );
        let white_on_red = contrast_ratio(super::text(), super::danger());
        assert!(
            white_on_red < AA,
            "白叉压红底 {white_on_red:.2}:1，仍不达标 ——\
             所以窗口关闭键的符号必须用 text_on_danger（黑叉）"
        );
    }

    #[test]
    fn accent_green_is_readable_as_text_on_dark_layers() {
        // 品牌绿的门槛提到 6:1，高于正文的 4.5:1。
        let ratio = contrast_ratio(super::accent(), super::panel());
        assert!(
            ratio >= 6.0,
            "品牌绿对纯黑侧栏应当≥6:1（当前 {ratio:.2}:1）。\
             调暗它会让界面失去 Spotify 的识别度"
        );
    }

    #[test]
    fn cards_sit_brighter_than_the_gap_around_them() {
        // 不变量是「卡片 vs 缝」：卡片必须比缝亮。
        let bg = relative_luminance(super::bg());
        let gap = relative_luminance(super::window_base());
        let chrome = relative_luminance(super::chrome());

        assert!(bg > gap + 1e-6, "主区卡片必须比缝亮");
        assert!(bg > chrome + 1e-6, "主区卡片必须比窗口外壳亮");

        // 三块卡片同色是有意的，锁住它以免侧栏被调回纯黑、让缝在那一侧失效。
        assert_eq!(
            super::panel(),
            super::bg(),
            "侧栏 / 右面板卡片应当与主区同色：分区交给缝，不交给明暗差"
        );
    }

    #[test]
    fn hero_tint_is_stable_and_distinguishable() {
        // 换掉哈希算法会让颜色每次重编译都变一遍，而界面上完全看不出这是 bug。
        assert_eq!(
            super::hero_tint(Some("城市漫游")),
            super::hero_tint(Some("城市漫游")),
            "同一个种子必须给出同一个颜色"
        );

        // 无种子时也必须是实色，`transparent` 会让整条渐变失效。
        let none = super::hero_tint(None);
        assert!(none.a > 0.9, "无种子时也必须是实色，否则渐变不生效");

        // 只有 6 个色相，撞色在预期内；这里只要求它不是常量函数。
        let colors: std::collections::HashSet<u32> = [
            "城市漫游",
            "夜航西飞",
            "Kind of Blue",
            "Abbey Road",
            "OK Computer",
            "Random Access Memories",
            "蓝",
            "a",
        ]
        .iter()
        .map(|s| {
            let c = super::hero_tint(Some(s));
            (c.r.to_bits()) ^ (c.g.to_bits() << 1) ^ (c.b.to_bits() << 2)
        })
        .collect();
        assert!(
            colors.len() >= 3,
            "种子不同却几乎总是同一个颜色，哈希退化了"
        );
    }

    #[test]
    fn status_colors_pass_aa() {
        assert_pass(super::danger(), super::bg(), "danger on bg");
        assert_pass(super::success(), super::bg(), "success on bg");
        assert_pass(super::danger(), super::panel(), "danger on panel");
        assert_pass(super::success(), super::panel(), "success on panel");
    }

    #[test]
    fn playing_row_is_readable() {
        // `accent_soft` 是半透明的，必须合成后再算，否则会高估。
        assert_pass(
            super::accent_text(),
            super::accent_soft(),
            "accent_text on accent_soft",
        );
    }

    #[test]
    fn settings_selection_is_readable_without_color() {
        // 分段控件里未选中的那格：次要文字压在比卡片更暗的轨道底（`bg`）上，必须达标。
        assert_pass(super::text_muted(), super::bg(), "未选中分段文字 on 轨道底");
        assert_pass(
            super::text_on_control(),
            super::control(),
            "选中分段文字 on 选中底",
        );
    }

    #[test]
    fn hover_row_stays_readable() {
        assert_pass(super::text(), super::hover(), "text on hover 行底");
        assert_pass(
            super::text_muted(),
            super::hover(),
            "text_muted on hover 行底",
        );
    }

    #[test]
    fn disabled_transport_button_is_readable() {
        // 空曲库时播放键是 `track()` 底 + 图标色；`text_faint` 压在上面只有 3.10:1。
        // 组合跨两个令牌，单看任一个都查不出来。
        assert_pass(super::text(), super::track(), "禁用播放键图标 on track 底");

        let faint = contrast_ratio(super::text_faint(), super::track());
        assert!(
            faint < AA,
            "text_faint 压在 track() 上只有 {faint:.2}:1，达不到 AA ——\
             禁用播放键的图标必须用 text()。如果这里失败了，说明 track() 被调暗了，\
             可以重新考虑用 text_faint"
        );
    }

    #[test]
    fn cursor_row_is_visibly_brighter_than_the_rest() {
        let hover = relative_luminance(super::hover());
        let cursor = relative_luminance(super::cursor());

        assert!(
            cursor > hover + 1e-6,
            "光标行必须比鼠标悬停行亮：两者同色时，鼠标停在 A 行、光标在 B 行就分不出来了"
        );

        // 门槛 1.25:1，高于 hover 对 bg 的 1.08:1。
        let ratio = contrast_ratio(super::cursor(), super::bg());
        assert!(
            ratio >= 1.25,
            "光标行底色对列表底只有 {ratio:.2}:1，在长列表里仍然找不出是哪一行"
        );

        // 行底变亮会牵动一整行文字，三档文字色都要重新过一遍 AA。
        assert_pass(super::text(), super::cursor(), "text on cursor 行底");
        assert_pass(
            super::text_muted(),
            super::cursor(),
            "text_muted on cursor 行底",
        );
        assert_pass(
            super::text_faint(),
            super::cursor(),
            "text_faint on cursor 行底",
        );
    }

    #[test]
    fn focus_ring_color_is_the_accent_token() {
        // `BoxShadow` 只吃 `Hsla`，所以环的颜色是手写的 HSL；它必须与 `accent_text` 是同一个绿。
        let ring: Rgba = super::focus_ring_color().into();
        let accent = super::accent_text();
        for (name, a, b) in [
            ("r", ring.r, accent.r),
            ("g", ring.g, accent.g),
            ("b", ring.b, accent.b),
        ] {
            assert!(
                (a - b).abs() < 0.01,
                "焦点环的 {name} 通道是 {a:.3}、`accent_text` 是 {b:.3} —— 两者应当是同一个绿"
            );
        }
    }

    #[test]
    fn focus_ring_is_visible_against_every_surface_it_lands_on() {
        // 焦点环不是文字，走 WCAG 1.4.11 的非文字门槛 3:1（正文是 4.5:1）。
        const NON_TEXT_AA: f32 = 3.0;
        for (name, surface) in [
            ("bg", super::bg()),
            ("panel", super::panel()),
            ("elevated", super::elevated()),
            ("chrome", super::chrome()),
        ] {
            let ratio = contrast_ratio(super::accent_text(), surface);
            assert!(
                ratio >= NON_TEXT_AA,
                "焦点环压在 {name} 上只有 {ratio:.2}:1，低于非文字对比度的 {NON_TEXT_AA}:1"
            );
        }
    }

    #[test]
    fn tag_text_is_readable_on_its_pill() {
        assert_pass(
            super::text_muted(),
            super::tag_bg(),
            "格式标签文字 on 标签底",
        );
    }

    #[test]
    fn luminance_endpoints_are_sane() {
        // 白 1.0 / 黑 0.0，对比度 21:1 —— 顺便验证公式本身没写反
        let white = rgb(0xffffff);
        let black = rgb(0x000000);
        assert!((relative_luminance(white) - 1.0).abs() < 1e-4);
        assert!(relative_luminance(black).abs() < 1e-6);
        assert!((contrast_ratio(white, black) - 21.0).abs() < 0.05);
    }
}
