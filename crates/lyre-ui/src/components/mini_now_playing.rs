//! 底栏左侧的迷你曲目信息。底栏是唯一常驻的区域，右面板收起后靠它显示当前曲目。

use std::sync::Arc;

use lyre_core::{AudioInfo, Quality};

use crate::common::menu::MenuTrigger;
use crate::cover::CoverCache;
use crate::entities::{LibraryEntity, PlaybackEntity, PlaybackEvent};
use crate::icon::{Glyph, icon, icon_inherit};
use crate::platform::*;
use crate::theme;

const COVER_SIZE: f32 = theme::MINI_COVER;

/// 用户点了迷你信息区，想去看完整信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiniPlayerEvent {
    /// 请求把右面板切到「正在播放」
    OpenNowPlaying,
}

pub struct MiniNowPlaying {
    playback: Entity<PlaybackEntity>,
    covers: Entity<CoverCache>,
    library: Entity<LibraryEntity>,
    track_title: String,
    track_subtitle: String,
    /// 当前曲目的音质档位。决定标题旁亮哪个徽章、还是干脆不亮。
    quality: Quality,
    /// 采样率 / 位深 / 码率，徽章点开就是看它。
    audio: AudioInfo,
    /// 当前曲目的格式（小写扩展名），徽章里作为「编码」一行列出。
    format: Option<String>,
    /// 当前曲目是否已收藏。爱心按钮据此画实心 / 空心。
    liked: bool,
    cover: Option<std::sync::Arc<RenderImage>>,
    cover_id: Option<lyre_core::TrackId>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<MiniPlayerEvent> for MiniNowPlaying {}

impl MiniNowPlaying {
    pub fn new(
        playback: Entity<PlaybackEntity>,
        covers: Entity<CoverCache>,
        library: Entity<LibraryEntity>,
        cx: &mut Context<Self>,
    ) -> Self {
        // 只订阅语义事件，不用 `observe`：播放期间进度通知是 10Hz，而 gpui 的一次 `notify` 让整棵树失效。
        let track_sub = cx.subscribe(&playback, |this, _, event: &PlaybackEvent, cx| {
            if matches!(event, PlaybackEvent::TrackChanged) {
                this.sync(cx);
            }
        });

        // 封面是 LRU 淘汰的，正在播放这首可能被挤出去，靠这里自愈而不是永久停在占位图上。
        let cover_sub = cx.observe(&covers, |this, _, cx| {
            this.refresh_cover(cx);
        });

        // 收藏态可能在别处翻转，所以要 observe 曲库，但只在真的变了才 notify：扫描期间曲库每 80ms notify 一次。
        let like_sub = cx.observe(&library, |this, library, cx| {
            let liked = this
                .cover_id
                .is_some_and(|id| library.read(cx).is_liked(id));
            if liked != this.liked {
                this.liked = liked;
                cx.notify();
            }
        });

        let mut this = Self {
            playback,
            covers,
            library,
            track_title: String::new(),
            track_subtitle: String::new(),
            quality: Quality::Lossy,
            audio: AudioInfo::default(),
            format: None,
            liked: false,
            cover: None,
            cover_id: None,
            _subs: vec![track_sub, cover_sub, like_sub],
        };
        this.sync(cx);
        this
    }

    /// 缓存里任何一张图解码完都会 notify，所以这里只问「正在播放这首的图换了没有」，
    /// 没换就不重绘 —— 否则滚动曲库时底栏每一张封面落地都要重排一次。
    fn refresh_cover(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.cover_id else {
            return;
        };
        let fresh = self.covers.read(cx).ready(id, COVER_SIZE);
        let changed = match (&self.cover, &fresh) {
            (Some(current), Some(candidate)) => !Arc::ptr_eq(current, candidate),
            (current, fresh) => current.is_some() != fresh.is_some(),
        };
        if !changed {
            return;
        }
        if fresh.is_none() && self.cover.is_some() {
            // 被挤出去了：重新发一次请求，等它解码完的那次 notify 把新图接上
            let path = App::update_entity(cx, &self.library, |library, _| {
                library.playlist().by_id(id).map(|track| track.path.clone())
            });
            if let Some(path) = path {
                self.covers
                    .update(cx, |cache, cx| cache.request(id, path, COVER_SIZE, cx));
            }
        }
        self.cover = fresh;
        cx.notify();
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let track = self.playback.read(cx).current().cloned();
        let Some(track) = track else {
            self.track_title.clear();
            self.track_subtitle.clear();
            self.quality = Quality::Lossy;
            self.audio = AudioInfo::default();
            self.format = None;
            self.liked = false;
            self.cover = None;
            self.cover_id = None;
            cx.notify();
            return;
        };

        track.title.clone_into(&mut self.track_title);
        self.track_subtitle = track.subtitle();
        self.format = track.format();
        self.audio = track.audio;
        self.quality = track.quality();
        self.liked = self.library.read(cx).is_liked(track.id);

        if track.has_cover {
            let covers = self.covers.clone();
            covers.update(cx, |cache, cx| {
                cache.request(track.id, track.path.clone(), COVER_SIZE, cx);
            });
        }
        self.cover = self.covers.read(cx).ready(track.id, COVER_SIZE);
        self.cover_id = Some(track.id);
        cx.notify();
    }
}

impl Render for MiniNowPlaying {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 封面可能在上一次 sync 之后才解码完，这里补一次读取
        if let Some(id) = self.cover_id
            && self.cover.is_none()
        {
            self.cover = self.covers.read(cx).ready(id, COVER_SIZE);
        }

        let art = div()
            .flex_none()
            .size(px(COVER_SIZE))
            .rounded(px(theme::RADIUS_LG))
            .overflow_hidden()
            .bg(theme::elevated())
            .flex()
            .items_center()
            .justify_center();

        let art = match self.cover.clone() {
            Some(image) => art.child(
                img(ImageSource::Render(image))
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            ),
            None => art.child(icon(Glyph::Music, 20., theme::text_faint())),
        };

        let has_track = self.cover_id.is_some();
        let title = if has_track {
            self.track_title.clone()
        } else {
            "未在播放".to_owned()
        };
        let subtitle = self.track_subtitle.clone();
        let liked = self.liked;
        let quality = self.quality;
        let audio = self.audio;
        let format = self.format.clone();
        let like_id = self.cover_id;

        let mut title_row = div()
            .flex()
            .items_center()
            .gap(px(theme::GAP_SM))
            .min_w_0()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(theme::FONT_TRACK))
                    .font_weight(theme::WEIGHT_BUTTON)
                    .text_color(if has_track {
                        theme::text()
                    } else {
                        theme::text_faint()
                    })
                    .child(title),
            );
        if has_track && quality != Quality::Lossy {
            title_row = title_row.child(quality_badge(quality, audio, format.clone()));
        }

        // 左侧可点区域：点它展开「正在播放」。爱心在这个区域之外，免得点击冒泡到这里。
        let info = div()
            .id("mini-info")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(theme::GAP_MD))
            .when(has_track, |row| {
                row.cursor_pointer()
                    .hover(|style| style.opacity(0.85))
                    .on_click(cx.listener(|_, _: &ClickEvent, _window, cx| {
                        cx.emit(MiniPlayerEvent::OpenNowPlaying);
                    }))
                    .tooltip(crate::common::tooltip::builder("查看正在播放"))
            })
            .child(art)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(1.))
                    .child(title_row)
                    .child(
                        div()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(theme::text_muted())
                            .truncate()
                            .child(subtitle),
                    ),
            );

        div()
            .id("mini-now-playing")
            .flex()
            .items_center()
            .gap(px(theme::GAP_SM))
            .min_w_0()
            .child(info)
            .when(has_track, |row| {
                row.child(heart_button(like_id, liked, &self.library))
            })
    }
}

/// 底栏左区的收藏爱心：无曲目时不画，有曲目时永远可见（不藏进 hover）。
fn heart_button(
    id: Option<lyre_core::TrackId>,
    liked: bool,
    library: &Entity<LibraryEntity>,
) -> AnyElement {
    let library = library.clone();
    div()
        .id("mini-like")
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme::RADIUS_PILL))
        .cursor_pointer()
        .text_color(if liked {
            theme::accent_text()
        } else {
            theme::text_muted()
        })
        .hover(|style| style.bg(theme::elevated()).text_color(theme::text()))
        .tooltip(crate::common::tooltip::builder(if liked {
            "取消收藏"
        } else {
            "收藏"
        }))
        .on_click(move |_, _, cx| {
            if let Some(id) = id {
                library.update(cx, |library, cx| {
                    library.toggle_like(id, cx);
                });
            }
        })
        .child(icon_inherit(
            if liked {
                Glyph::HeartFilled
            } else {
                Glyph::Heart
            },
            16.,
        ))
        .into_any_element()
}

/// 徽章上的字。有损由调用方挡掉，这里只剩两档。
fn badge_label(quality: Quality) -> &'static str {
    if quality == Quality::HiRes {
        "HI-RES"
    } else {
        "LOSSLESS"
    }
}

/// 悬停提示：有规格时只说「点这里看」，数字留给浮层，免得两处重复。
fn badge_tip(audio: AudioInfo, format: Option<&str>) -> String {
    if !audio.is_empty() {
        return "点击查看音质详情".to_owned();
    }
    let codec = format.unwrap_or_default().to_uppercase();
    if codec.is_empty() {
        "无损音频".to_owned()
    } else {
        format!("无损音频（{codec}）")
    }
}

/// 音质徽章：悬停给提示，点开给逐项规格。
/// 什么规格都没读到时仍然亮徽章（「无损」是编码事实，不依赖规格），但不挂浮层。
fn quality_badge(quality: Quality, audio: AudioInfo, format: Option<String>) -> AnyElement {
    let codec = format.unwrap_or_default().to_uppercase();
    let tip = badge_tip(audio, Some(codec.as_str()));

    let badge = div()
        .id("mini-quality")
        .flex_none()
        .px(px(theme::GAP_XS + 2.0))
        .py(px(1.))
        .rounded(px(theme::RADIUS_SM))
        .border_1()
        .border_color(theme::accent())
        .text_size(px(theme::FONT_TAG))
        .font_weight(theme::WEIGHT_BUTTON)
        .text_color(theme::accent_text())
        .cursor_pointer()
        // 徽章落在「点开正在播放」的可点区域里，不拦下来会连面板一起弹开
        .on_click(|_, _, cx| cx.stop_propagation())
        .tooltip(crate::common::tooltip::builder(tip))
        .child(badge_label(quality));

    if audio.is_empty() {
        return badge.into_any_element();
    }

    let mut rows = audio.details();
    if !codec.is_empty() {
        rows.push(("编码", codec));
    }

    Popover::new("mini-quality-detail")
        // 底栏贴在窗口最下沿，浮层只能朝上开
        .anchor(Anchor::BottomLeft)
        .arrow(true)
        .trigger(MenuTrigger::new(badge))
        .content(move |_, _, _| quality_detail(rows.clone()))
        .into_any_element()
}

fn quality_detail(rows: Vec<(&'static str, String)>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_XS))
        .min_w(px(168.))
        .children(rows.into_iter().map(|(label, value)| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(theme::GAP_LG))
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .text_color(theme::text_muted())
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(theme::FONT_CAPTION))
                        .font_weight(theme::WEIGHT_BUTTON)
                        .text_color(theme::text())
                        .child(value),
                )
        }))
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*;`：`crate::platform::*` 里的 gpui `test` 过程宏会遮蔽内置 `#[test]`。
    use super::{badge_label, badge_tip};
    use lyre_core::{AudioInfo, Quality};

    #[test]
    fn 徽章字样只有两档() {
        assert_eq!(badge_label(Quality::HiRes), "HI-RES");
        assert_eq!(badge_label(Quality::Lossless), "LOSSLESS");
    }

    #[test]
    fn 提示只在没有详情可弹时才报规格() {
        let audio = AudioInfo {
            sample_rate: Some(96000),
            bit_depth: Some(24),
            bitrate: Some(2834),
        };
        // 这些数字浮层里会出现一遍，tooltip 再写一遍就成了重复（浮层展开时两者同屏）
        assert_eq!(badge_tip(audio, Some("flac")), "点击查看音质详情");
        // 读不到规格时没有浮层可点，tooltip 就是唯一的说明
        assert_eq!(
            badge_tip(AudioInfo::default(), Some("wav")),
            "无损音频（WAV）"
        );
        assert_eq!(badge_tip(AudioInfo::default(), None), "无损音频");
    }
}
