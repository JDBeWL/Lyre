//! 歌词全屏视图：盖在主区卡片上，当前句高亮居中、逐字点亮、点击跳转。
//! 跟随进度重绘的粒度是「换行或换字」，不是每帧。

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use lyre_core::{LyricLine, Lyrics};

use crate::entities::PlaybackEntity;
use crate::icon::Glyph;
use crate::platform::*;
use crate::shell::ShellEntity;
use crate::theme;

/// 文字盒高 = 字号 × 这个比例。gpui 默认行高是字号的 1.618 倍，用它会让原文盒（52）远大于译文盒。
const LINE_BOX_RATIO: f32 = 1.3;
/// 药丸的上下内边距，左右用 `GAP_LG`。
const PILL_PAD_Y: f32 = theme::GAP_XS;
/// 相邻两行药丸之间的空隙。
const ROW_GAP: f32 = theme::GAP_SM;
/// 整列共用的基准字号档位，从大到小取第一个放得下典型长句的一档。
const BASE_TIERS: [f32; 3] = [theme::FONT_HERO, 26.0, 22.0];
/// 最小基准字号，必须与 `BASE_TIERS` 末项一致。
const BASE_FLOOR: f32 = 22.0;
const _: () = assert!(BASE_FLOOR == BASE_TIERS[BASE_TIERS.len() - 1]);
/// 挑档位时按多长的句子算「放得下」。再长的让它折行，而不是把整列拖小。
const LONG_LINE_EM: f32 = 20.0;
/// 邻行相对当前行的字号比例。用比例而非固定 px 差：基准降到 22 时再减 8 就只剩 14。
const IDLE_RATIO: f32 = 0.7;
/// 邻行字号的可读下限。基准最低 22，`22 × 0.7 = 15.4` 会撞到它。
const IDLE_FLOOR: f32 = 16.0;
/// 译文相对原文的固定字号差。
const TRANSLATION_STEP: f32 = 4.0;
/// 邻行按距离衰减的不透明度 —— gpui 的元素样式没有 `blur`，用它代替雾化。
const OPACITY_NEAR: f32 = 0.5;
const OPACITY_FAR: f32 = 0.22;
/// 当前行里还没唱到的字的不透明度，必须高于邻行。
const OPACITY_UNSUNG: f32 = 0.6;
const _: () = assert!(OPACITY_UNSUNG > OPACITY_NEAR);
/// 逐字亮度爬升时长的夹取区间。
const RAMP_MIN: Duration = Duration::from_millis(60);
const RAMP_MAX: Duration = Duration::from_millis(900);

/// 弹簧参数：略低于临界阻尼，一步一行的滚动不来回晃。
const SCROLL_SPRING: SpringConfig = SpringConfig::new(180.0, 26.0, 1.0);

#[derive(Clone, Debug)]
enum Loaded {
    /// 什么都没装载
    Idle,
    /// 正在后台读侧文件
    Loading,
    Ready(Rc<Lyrics>),
    /// 确认没有同名 `.lrc` / `.ass`，或读不出内容
    Missing,
}

pub struct LyricsView {
    playback: Entity<PlaybackEntity>,
    shell: Entity<ShellEntity>,
    state: Loaded,
    /// `state` 属于哪首曲目。换曲靠它识别，也用来丢弃过期请求的结果。
    path: Option<PathBuf>,
    /// 当前行 / 当前字。只在它们变化时 `notify`，把跟随进度的重绘压到每句每字一次。
    active: Option<usize>,
    word: Option<usize>,
    _subs: Vec<Subscription>,
}

impl LyricsView {
    pub fn new(
        playback: Entity<PlaybackEntity>,
        shell: Entity<ShellEntity>,
        cx: &mut Context<Self>,
    ) -> Self {
        // observe 而不是 subscribe：进度本身不发事件，只会 `notify` 播放实体。
        let observe = cx.observe(&playback, |this, _, cx| this.on_playback(cx));
        Self {
            playback,
            shell,
            state: Loaded::Idle,
            path: None,
            active: None,
            word: None,
            _subs: vec![observe],
        }
    }

    /// 播放实体每次 `notify` 都会走到这里（约 10Hz），但只有换曲、换行、换字才可能重绘。
    fn on_playback(&mut self, cx: &mut Context<Self>) {
        let current = self
            .playback
            .read(cx)
            .current()
            .map(|track| track.path.clone());

        match current {
            Some(path)
                if self.path.as_deref() != Some(path.as_path())
                    || matches!(self.state, Loaded::Idle) =>
            {
                self.request(path, cx);
            }
            None if !matches!(self.state, Loaded::Idle) => {
                self.path = None;
                self.state = Loaded::Idle;
                self.active = None;
                self.word = None;
                cx.notify();
            }
            _ => {}
        }

        self.sync_active(cx);
    }

    /// 后台读侧文件。发起时不 `notify`：遮罩要么还没打开，要么显示的仍是上一句。
    fn request(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.path = Some(path.clone());
        self.state = Loaded::Loading;
        self.active = None;
        self.word = None;

        // `background_executor()` 返回借用，必须 clone 才能带进 'static 任务
        let executor = cx.background_executor().clone();
        let loaded_for = path.clone();
        cx.spawn(async move |this, cx| {
            let loaded = executor
                .spawn(async move { lyre_core::lyrics::load_for_audio(&loaded_for) })
                .await;
            this.update(cx, |this, cx| {
                // 期间又换过曲就丢弃这份结果，否则会把上一首的歌词贴到当前这首上
                if this.path.as_deref() == Some(path.as_path()) {
                    this.state = match loaded {
                        Some(lyrics) => Loaded::Ready(Rc::new(lyrics)),
                        None => Loaded::Missing,
                    };
                    this.active = None;
                    this.word = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// 重算当前行与当前字；遮罩关着就不动，两者都没变也不重绘。
    fn sync_active(&mut self, cx: &mut Context<Self>) {
        if !self.shell.read(cx).shows_lyrics() {
            return;
        }
        let Loaded::Ready(lyrics) = &self.state else {
            return;
        };
        let position = self.playback.read(cx).position();
        let next = lyrics.active_index(position);
        let next_word = next.and_then(|index| lyrics.lines[index].word_index(position));
        if next != self.active || next_word != self.word {
            self.active = next;
            self.word = next_word;
            cx.notify();
        }
    }
}

/// 点击任意一行 → 跳到它的起始时刻。
fn seek_handler(
    playback: &Entity<PlaybackEntity>,
    start: Duration,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let playback = playback.clone();
    move |_, _, cx| playback.update(cx, |playback, _| playback.seek(start))
}

/// 一行在给定字号下的估计宽度：CJK / 假名 / 全角标点 ≈ 1 em，其余 ASCII ≈ 0.52 em，
/// 空格 0.30 em，粗体 ×1.06。只用来数折行的行数，估大比估小安全。
fn text_width(text: &str, size: f32, bold: bool) -> f32 {
    let em = text
        .chars()
        .map(|c| match c {
            ' ' => 0.30,
            c if c < '\u{2E80}' => 0.52,
            _ => 1.0,
        })
        .sum::<f32>();
    em * size * if bold { 1.06 } else { 1.0 }
}

/// 正文可用的文字宽度。gpui 渲染期拿不到元素实际宽度，只能从布局状态推：
/// 卡片宽 = 窗口宽 − 两侧 GUTTER − 侧栏 −（右面板开着时的缝 + 面板宽），再扣掉内边距。
fn text_budget(window: f32, sidebar: f32, panel_open: bool) -> f32 {
    let panel = if panel_open {
        theme::GUTTER + theme::RIGHT_PANEL_W
    } else {
        0.0
    };
    (window - 2.0 * theme::GUTTER - sidebar - panel - 2.0 * theme::GAP_XL - 2.0 * theme::GAP_LG)
        .max(0.0)
}

/// 整列的基准字号：只看正文预算，不看每句长短（签名里没有句子）。
fn active_size(budget: f32) -> f32 {
    *BASE_TIERS
        .iter()
        .find(|size| LONG_LINE_EM * **size <= budget)
        .unwrap_or(&BASE_FLOOR)
}

/// 邻行的字号：基准退一档比例，夹在可读下限上。
fn idle_size(budget: f32) -> f32 {
    (active_size(budget) * IDLE_RATIO).round().max(IDLE_FLOOR)
}

/// 一行文字在给定字号下要占几个视觉行。
fn visual_rows(text: &str, size: f32, budget: f32, bold: bool) -> usize {
    (text_width(text, size, bold) / budget.max(1.0))
        .ceil()
        .max(1.0) as usize
}

/// 译文跟着原文缩，只固定差 [`TRANSLATION_STEP`] px。
fn translation_size(original: f32) -> f32 {
    original - TRANSLATION_STEP
}

/// 一个文字盒的高度，同时用作 `line_height` 和 `h`，所以布局不依赖字体度量：
/// gpui 把墨迹在 `line_height` 里上下均分，盒高按字号等比才能让各行墨迹中心等距。
fn line_box(size: f32) -> f32 {
    (size * LINE_BOX_RATIO).round()
}

/// 译文占几行。每行都画自己的译文，所以行高不再统一。
fn translation_rows(line: &LyricLine) -> Vec<&str> {
    line.translation
        .as_deref()
        .map(|text| text.lines().filter(|row| !row.trim().is_empty()).collect())
        .unwrap_or_default()
}

/// 药丸内容区高度：原文 + 它自己的每行译文，各自按 `size` 数出要占几个视觉行。
/// 译文永远按常规字重画，粗体只影响原文行。
fn pill_height(line: &LyricLine, size: f32, budget: f32, bold: bool) -> f32 {
    let trans = translation_size(size);
    visual_rows(&line.text, size, budget, bold) as f32 * line_box(size)
        + translation_rows(line)
            .iter()
            .map(|row| visual_rows(row, trans, budget, false) as f32 * line_box(trans))
            .sum::<f32>()
}

/// 一行的槽位高度：按「这一行是当前行」的字号与粗体算，槽位才永远装得下药丸；
/// 且不随谁是当前行变化，否则弹簧每步位移不等距，滚动会一长一短地跳。
fn slot_height(line: &LyricLine, budget: f32) -> f32 {
    pill_height(line, active_size(budget), budget, true) + 2.0 * PILL_PAD_Y + ROW_GAP
}

/// 当前行的中心离正文列顶部多远，列从「容器正中」的锚点往上挪这个距离。
/// 不用外层 flex 居中：列比容器高得多，溢出的 flex 子项怎么居中由布局引擎的 overflow 对齐决定。
fn active_center_from_top(heights: &[f32], active: usize) -> f32 {
    let Some(last) = heights.len().checked_sub(1) else {
        return 0.0;
    };
    let active = active.min(last);
    heights[..active].iter().sum::<f32>() + heights[active] / 2.0
}

impl Render for LyricsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let track = self.playback.read(cx).current().cloned();
        let seed = track
            .as_ref()
            .and_then(|track| track.album.clone())
            .or_else(|| track.as_ref().map(|track| track.title.clone()));
        let tint = theme::hero_tint(seed.as_deref());
        let active = self.active;
        let word = self.word;
        let budget = {
            let shell = self.shell.read(cx);
            text_budget(
                shell.width(),
                shell.sidebar().width(),
                shell.right_panel().is_some(),
            )
        };

        let body = match (&self.state, track.as_ref()) {
            (Loaded::Ready(lyrics), Some(_)) => {
                lyrics_column(lyrics, active, word, &self.playback, budget).into_any_element()
            }
            (Loaded::Loading, _) => notice("歌词读取中…", None).into_any_element(),
            (_, None) | (Loaded::Idle, _) => {
                notice("没有在播放的歌", Some("先播放一首，歌词会跟着进度往上滚"))
                    .into_any_element()
            }
            (Loaded::Missing, Some(_)) => notice(
                "这首歌没有歌词",
                Some("把同名的 .lrc 或 .ass 放在音频旁边，例如 song.lrc / song.ass"),
            )
            .into_any_element(),
        };

        div()
            // 遮罩必须绑鼠标监听：gpui 只对注册了鼠标监听的元素做命中测试，光有 `id` 不挡点击。
            .id("lyrics-overlay")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme::bg())
            .px(px(theme::GAP_XL))
            .pt(px(theme::GAP_XL))
            .pb(px(theme::GAP_LG))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .bg(linear_gradient(
                        0.0,
                        linear_color_stop(tint.opacity(0.0), 0.0),
                        linear_color_stop(tint.opacity(0.5), 1.0),
                    )),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .bg(linear_gradient(
                        90.0,
                        linear_color_stop(tint.opacity(0.22), 0.0),
                        linear_color_stop(tint.opacity(0.0), 1.0),
                    )),
            )
            .child(header(
                track.as_ref().map(|track| track.title.as_str()),
                track.as_ref().and_then(|track| track.artist.as_deref()),
                self.shell.clone(),
            ))
            .child(div().flex_1().min_h_0().flex().child(body))
    }
}

/// 顶部一行：曲目身份 + 关闭按钮。
fn header(
    title: Option<&str>,
    artist: Option<&str>,
    shell: Entity<ShellEntity>,
) -> impl IntoElement {
    let close = crate::common::button::IconButton::new("lyrics-close", Glyph::Cross)
        .label("关闭歌词")
        .box_size(32.)
        .glyph_size(15.)
        .color(theme::text_muted())
        .on_click(move |_, _, cx| shell.update(cx, ShellEntity::toggle_lyrics))
        .render();

    div()
        .flex_none()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(theme::GAP_MD))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .text_size(px(theme::FONT_MICRO))
                        .text_color(theme::text_faint())
                        .child("歌词"),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(theme::FONT_SUBTITLE))
                        .font_weight(theme::WEIGHT_TITLE)
                        .text_color(theme::text())
                        .truncate()
                        .child(title.unwrap_or("未在播放").to_owned()),
                )
                .when_some(artist, |info, artist| {
                    info.child(
                        div()
                            .flex_none()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(theme::text_muted())
                            .truncate()
                            .child(artist.to_owned()),
                    )
                }),
        )
        .child(close)
}

/// 居中一块提示文字：加载中 / 没有歌词 / 没在播放。
fn notice(headline: &'static str, detail: Option<&'static str>) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::GAP_SM))
        .child(
            div()
                .flex_none()
                .text_size(px(theme::FONT_SUBTITLE))
                .text_color(theme::text_muted())
                .child(headline),
        )
        .when_some(detail, |block, detail| {
            block.child(
                div()
                    .flex_none()
                    .max_w(px(420.))
                    .text_center()
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(theme::text_faint())
                    .child(detail),
            )
        })
}

/// 歌词正文：每行一个固定槽位（见 [`slot_height`]），药丸在槽位里居中，
/// 靠弹簧偏移把当前行送到容器垂直中央。
fn lyrics_column(
    lyrics: &Rc<Lyrics>,
    active: Option<usize>,
    word: Option<usize>,
    playback: &Entity<PlaybackEntity>,
    budget: f32,
) -> impl IntoElement {
    let anchor = active.unwrap_or(0);
    // 整列共用一个字号：邻行只是它退一档，所以两档都先算一次
    let current = active_size(budget);
    let idle = idle_size(budget);
    let heights: Vec<f32> = lyrics
        .lines
        .iter()
        .map(|l| slot_height(l, budget))
        .collect();

    let rows = lyrics.lines.iter().enumerate().map(|(index, line)| {
        let is_active = active == Some(index);
        let distance = if is_active {
            0
        } else {
            index.abs_diff(anchor).max(1)
        };
        let opacity = match distance {
            0 => 1.0,
            1..=2 => OPACITY_NEAR,
            _ => OPACITY_FAR,
        };
        let size = if is_active { current } else { idle };
        let original_box = line_box(size);

        let original = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_center()
            // 放不下的长句折行，所以宽度封顶在正文预算上（行数按同一个预算数出来）。
            .max_w(px(budget))
            .line_height(px(original_box))
            .text_size(px(size))
            .font_weight(if is_active {
                theme::WEIGHT_TITLE
            } else {
                theme::WEIGHT_TRACK
            })
            .text_color(theme::text())
            .children(original_spans(line, index, is_active, word, opacity));

        // 药丸按内容收缩；槽位高度只用于滚动偏移。
        let group = SharedString::from(format!("lyric-row-{index}"));
        let mut pill = div()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .px(px(theme::GAP_LG))
            .py(px(PILL_PAD_Y))
            .rounded(px(theme::RADIUS_LG))
            .group_hover(group.clone(), |style| style.bg(theme::hover()))
            .child(original);

        for part in translation_rows(line) {
            let trans_size = translation_size(size);
            let trans_box = line_box(trans_size);
            pill = pill.child(
                div()
                    .flex_none()
                    .max_w(px(budget))
                    .text_center()
                    .line_height(px(trans_box))
                    .opacity(opacity)
                    .text_size(px(trans_size))
                    .text_color(theme::text_muted())
                    .child(part.to_owned()),
            );
        }

        div()
            .id(SharedString::from(format!("lyric-{index}")))
            .group(group)
            .flex_none()
            .w_full()
            .h(px(heights[index]))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_center()
            .cursor_pointer()
            .on_click(seek_handler(playback, line.start))
            .child(pill)
    });

    let offset = active_center_from_top(&heights, anchor);

    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .relative()
        .overflow_hidden()
        // 容器正中的 0 高锚点：列从它往上挪，绕开「居中溢出盒子」的语义
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(relative(0.5))
                .h(px(0.0))
                .child(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .items_center()
                        .w_full()
                        .min_w_0()
                        // 弹簧自己补帧，视图不必每帧重绘；目标只在换行时变
                        .with_spring(
                            "lyrics-scroll",
                            SpringAnimation::new(SCROLL_SPRING).to(px(-offset)),
                            Styled::top,
                        )
                        .children(rows),
                ),
        )
}

/// 当前行的正文：有逐字时间戳就一格一个元素，正在唱的那格在自己的时长里从暗爬到亮；
/// 没有逐字信息时整行一个元素。
fn original_spans(
    line: &LyricLine,
    row: usize,
    is_active: bool,
    word: Option<usize>,
    opacity: f32,
) -> Vec<AnyElement> {
    if !is_active || line.words.is_empty() {
        return vec![
            div()
                .flex_none()
                .opacity(opacity)
                .child(line.text.clone())
                .into_any_element(),
        ];
    }
    let sung = word.unwrap_or(0);
    line.words
        .iter()
        .enumerate()
        .map(|(index, part)| {
            if index < sung {
                return div()
                    .flex_none()
                    .opacity(1.0)
                    .child(part.text.clone())
                    .into_any_element();
            }
            if index > sung {
                return div()
                    .flex_none()
                    .opacity(OPACITY_UNSUNG)
                    .child(part.text.clone())
                    .into_any_element();
            }
            // 动画 id 带格号：换格即重挂，这一格的爬升恰好在自己唱完时到顶。
            // 时间戳倒挂（脏歌词）时按最短爬升处理，不给 panic 留口子。
            let ramp = part
                .end
                .checked_sub(part.start)
                .unwrap_or_default()
                .clamp(RAMP_MIN, RAMP_MAX);
            div()
                .flex_none()
                .child(part.text.clone())
                .with_animation(
                    SharedString::from(format!("lyric-lit-{row}-{index}")),
                    Animation::new(ramp),
                    |span, delta| span.opacity(OPACITY_UNSUNG + (1.0 - OPACITY_UNSUNG) * delta),
                )
                .into_any_element()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        LINE_BOX_RATIO, PILL_PAD_Y, RAMP_MAX, RAMP_MIN, ROW_GAP, TRANSLATION_STEP,
        active_center_from_top, active_size, idle_size, line_box, pill_height, slot_height,
        text_budget, text_width, translation_rows, translation_size, visual_rows,
    };
    use crate::theme;
    use lyre_core::{LyricLine, LyricWord};

    const H: f32 = 60.0;
    const T: f32 = 24.0;

    fn line(text: &str, translation: Option<&str>, words: usize) -> LyricLine {
        LyricLine {
            start: Duration::ZERO,
            end: Duration::from_secs(4),
            text: text.to_owned(),
            translation: translation.map(str::to_owned),
            words: (0..words)
                .map(|index| LyricWord {
                    start: Duration::from_millis(index as u64 * 200),
                    end: Duration::from_millis(index as u64 * 200 + 200),
                    text: "字".to_owned(),
                })
                .collect(),
        }
    }

    /// 宽到够得着最大档的预算（1920px 最大化窗口实测约 1570px）。
    const WIDE: f32 = 1500.0;
    /// 最小窗口（560 宽、侧栏收成图标条）时的预算。
    const NARROW: f32 = 150.0;

    /// 608 = 1264px 窗口开着右面板，355 = 1011px。
    #[test]
    fn 字号只看预算不看句子() {
        for budget in [WIDE, 900.0, 608.0, 355.0, NARROW, 0.0] {
            let (current, idle) = (active_size(budget), idle_size(budget));
            assert!(
                (22.0..=theme::FONT_HERO).contains(&current),
                "预算 {budget} 时整列字号 {current} 越界"
            );
            assert!(
                current - idle >= 4.0,
                "预算 {budget}：当前句 {current}、邻句 {idle}，差 {} 看不出来",
                current - idle
            );
        }
        assert_eq!(active_size(WIDE), theme::FONT_HERO);
        assert!(
            active_size(NARROW) < active_size(WIDE),
            "窄窗口必须降一档，否则整屏都在折行"
        );
    }

    /// 放不下的长句折行，所以槽位要按「几个视觉行」留高 —— 只留一行高就会把第二行裁掉。
    #[test]
    fn 长句按行数占位() {
        let budget = 608.0;
        let size = active_size(budget);
        assert_eq!(
            visual_rows(&"字".repeat(30), size, budget, true),
            2,
            "30 em 在 {budget}px 里放不下两行都不止"
        );
        let short = line("短句", None, 0);
        let long = line(&"字".repeat(30), None, 0);
        assert!(
            slot_height(&long, budget) >= 2.0 * line_box(size) + 2.0 * PILL_PAD_Y + ROW_GAP,
            "两行的句子只留了一行高：{}",
            slot_height(&long, budget)
        );
        assert!(slot_height(&long, budget) > slot_height(&short, budget));
    }

    /// 译文常常比原文长得多，它也要按自己的字号数行数，不能只按原文留高。
    #[test]
    fn 译文行按自己的字号数行数() {
        let budget = 608.0;
        let size = active_size(budget);
        let trans = translation_size(size);
        let plain = line("答えは もう", None, 0);
        let bilingual = line("答えは もう", Some(&"我的回答是的".repeat(6)), 0);
        assert!(
            slot_height(&bilingual, budget) >= slot_height(&plain, budget) + line_box(trans),
            "长译文没多占行数：{} vs {}",
            slot_height(&bilingual, budget),
            slot_height(&plain, budget)
        );
    }

    #[test]
    fn 宽度估算按全角半角分档() {
        assert!(text_width("ああああ", 20.0, false) > text_width("aaaa", 20.0, false));
        assert!(text_width("    ", 20.0, false) < text_width("aaaa", 20.0, false));
        assert!(text_width("同一句", 20.0, true) > text_width("同一句", 20.0, false));
        assert_eq!(text_width("", 32.0, true), 0.0, "空行不该占宽度");
    }

    /// 预算的每一项都对应 root.rs 里的一处真实留白，少扣一处就会把句尾裁掉。
    #[test]
    fn 预算从窗口宽一路扣到正文() {
        let wide = text_budget(1920.0, theme::SIDEBAR_W, false);
        assert!(
            wide > 1500.0,
            "最大化窗口该给正文留下 1500px 以上，实测 {wide}"
        );
        assert!(
            text_budget(1920.0, theme::SIDEBAR_W, true) + theme::GUTTER + theme::RIGHT_PANEL_W
                == wide,
            "右面板展开时要正好扣掉一格面板宽"
        );
        assert!(
            text_budget(560.0, theme::SIDEBAR_ICON_W, true) > 0.0,
            "最小窗口不能扣成负数"
        );
        assert_eq!(
            text_budget(100.0, theme::SIDEBAR_W, true),
            0.0,
            "扣穿了要夹到 0"
        );
    }

    #[test]
    fn 译文只比原文小四像素() {
        for budget in [WIDE, 900.0, 608.0, 355.0, NARROW] {
            let original = active_size(budget);
            assert_eq!(
                translation_size(original),
                original - TRANSLATION_STEP,
                "预算 {budget} 时译文差得不对"
            );
            assert!(
                translation_size(idle_size(budget)) >= 12.0,
                "邻句的译文小到读不清：{}",
                translation_size(idle_size(budget))
            );
        }
    }

    #[test]
    fn 逐字亮度的爬升落在可读区间() {
        assert!(RAMP_MIN >= Duration::from_millis(40), "太短就还是硬切");
        assert!(
            RAMP_MAX <= Duration::from_millis(1_200),
            "太长会看起来迟迟不亮"
        );
        assert!(RAMP_MIN < RAMP_MAX);
    }

    #[test]
    fn 均匀行高时中心距离就是行号加半格() {
        for count in 1..10 {
            let heights = vec![H; count];
            for active in 0..count {
                assert_eq!(
                    active_center_from_top(&heights, active),
                    (active as f32 + 0.5) * H,
                    "{count} 行第 {active} 行的锚距不对"
                );
            }
        }
        assert_eq!(active_center_from_top(&[], 3), 0.0, "空列不能 panic");
        // active 越界（换曲瞬间的旧序号）要夹住，不能读越界
        let heights = vec![H; 3];
        assert_eq!(active_center_from_top(&heights, 99), 2.5 * H);
    }

    #[test]
    fn 换行时锚距按上一行的实际高度推进() {
        let uniform = vec![H; 8];
        for active in 0..7 {
            assert_eq!(
                active_center_from_top(&uniform, active + 1)
                    - active_center_from_top(&uniform, active),
                H,
                "行高统一时换一句恰好走一行高，弹簧每步位移才等距"
            );
        }
        // 上一行更高时，换到下一行要多走它的整高，而不是一个固定行高
        let mut mixed = vec![H; 8];
        mixed[3] = H + T;
        assert_eq!(
            active_center_from_top(&mixed, 4) - active_center_from_top(&mixed, 3),
            H + T / 2.0
        );
    }

    #[test]
    fn 当前行下方的行不影响它是否居中() {
        // 锚点法只看「列顶到当前行中心」的距离，所以下面的行长成什么样都无所谓
        let base = vec![H; 7];
        let mut below = base.clone();
        below[5] += 3.0 * T;
        assert_eq!(
            active_center_from_top(&base, 2),
            active_center_from_top(&below, 2)
        );

        // 上方的行变高，当前行就被顶下去它的整高
        let mut above = base.clone();
        above[1] += T;
        assert_eq!(
            active_center_from_top(&above, 2) - active_center_from_top(&base, 2),
            T
        );

        // 当前行自己变高只算一半：要的是中心对齐，不是顶对齐
        let mut own = base.clone();
        own[2] += T;
        assert_eq!(
            active_center_from_top(&own, 2) - active_center_from_top(&base, 2),
            T / 2.0
        );
    }

    #[test]
    fn 槽位包含自己的译文行且永远装得下药丸() {
        let plain = line("原文原文", None, 0);
        let bilingual = line("原文原文", Some("译文译文"), 0);
        let trans_box = line_box(translation_size(active_size(WIDE)));
        assert!(
            slot_height(&bilingual, WIDE) > slot_height(&plain, WIDE),
            "多一行译文就要多占一行"
        );
        assert_eq!(
            slot_height(&bilingual, WIDE) - slot_height(&plain, WIDE),
            trans_box,
            "译文行按自己的字号加高"
        );

        let three_languages = line("原文", Some("译文一\n译文二"), 0);
        assert_eq!(translation_rows(&three_languages), ["译文一", "译文二"]);
        assert_eq!(
            translation_rows(&line("原文", Some("  \n \n"), 0)),
            [] as [&str; 0]
        );
        assert_eq!(
            slot_height(&three_languages, WIDE) - slot_height(&line("原文", None, 0), WIDE),
            2.0 * trans_box
        );

        // 槽位按「当前行」的字号与行数算，邻行（更小字号、更少行数）永远装得下
        for chars in [3usize, 14, 15, 24, 25, 60] {
            let text = "字".repeat(chars);
            for translation in [None, Some("译文译文"), Some(&"译".repeat(50) as &str)] {
                let row = line(&text, translation, 0);
                for budget in [WIDE, NARROW, 320.0, 608.0] {
                    for (size, bold) in [(active_size(budget), true), (idle_size(budget), false)] {
                        let pill = pill_height(&row, size, budget, bold) + 2.0 * PILL_PAD_Y;
                        assert!(
                            pill + ROW_GAP <= slot_height(&row, budget),
                            "{chars} 字 / 译文 {translation:?} / 预算 {budget} / 字号 {size} 顶出槽位：{pill} > {}",
                            slot_height(&row, budget) - ROW_GAP
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn 双语行的墨迹中心落在药丸中心() {
        // 墨迹在各自的盒子里居中，联合中心按墨迹高度加权
        let ink_center = |boxes: &[f32], sizes: &[f32]| -> f32 {
            let mut tops = vec![0.0];
            for box_h in &boxes[..boxes.len() - 1] {
                let next = tops.last().unwrap() + box_h;
                tops.push(next);
            }
            let inks: Vec<f32> = sizes.iter().map(|size| size * 0.9).collect();
            let total: f32 = inks.iter().sum();
            inks.iter()
                .zip(&tops)
                .zip(boxes)
                .map(|((ink, top), box_h)| ink * (top + box_h / 2.0))
                .sum::<f32>()
                / total
        };
        let middle = |boxes: &[f32]| boxes.iter().sum::<f32>() / 2.0;

        let size = active_size(WIDE);
        let sizes = [size, translation_size(size)];
        let boxes = [line_box(sizes[0]), line_box(sizes[1])];
        assert!(
            (ink_center(&boxes, &sizes) - middle(&boxes)).abs() < 0.5,
            "等比盒高的墨迹中心 {} 应等于内容中心 {}",
            ink_center(&boxes, &sizes),
            middle(&boxes)
        );

        let old = [sizes[0] * 1.618, 26.0];
        assert!(
            ink_center(&old, &sizes) > middle(&old) + 3.0,
            "固定译文盒应把墨迹压低（旧 bug）"
        );
    }

    #[test]
    fn 文字盒随字号等比缩放() {
        assert_eq!(line_box(20.0), (20.0 * LINE_BOX_RATIO).round());
        assert!(line_box(28.0) > 26.0, "28px 的译文盒不能矮过旧的固定 26px");
        assert!(line_box(32.0) > line_box(28.0));
    }

    #[test]
    fn 未唱到的字比唱过的淡() {
        let karaoke = line("甲乙", None, 3);
        assert_eq!(karaoke.word_index(Duration::ZERO), Some(0));
        assert_eq!(karaoke.word_index(Duration::from_millis(400)), Some(2));
        // 行尾之后仍停在最后一格，不会翻到下一格
        assert_eq!(karaoke.word_index(Duration::from_secs(9)), Some(2));
        // 没有逐字信息的行返回 None，界面整行一起点亮
        assert_eq!(line("甲乙", None, 0).word_index(Duration::ZERO), None);
    }
}
