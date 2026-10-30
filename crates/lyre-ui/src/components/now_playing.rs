//! 右侧「正在播放」面板：封面 + 标题 + 关于艺术家 + 制作人员 + 下一首。
//! 封面按需加载：曲目变化时才读标签、解码，结果落到 `CoverCache`。

use std::sync::Arc;

use lyre_core::{Track, UNKNOWN_ARTIST, artist_key_str, format_duration};

use crate::common::thousands;
use crate::cover::CoverCache;
use crate::entities::{LibraryEntity, PlaybackEntity, PlaybackEvent};
use crate::icon::Glyph;
use crate::platform::*;
use crate::shell::ShellEntity;
use crate::theme;

use super::panel_close_button;

const COVER_SIZE: f32 = 200.0;
/// 「下一首」卡片里的小封面边长。
const UP_NEXT_COVER: f32 = 44.0;

pub struct NowPlaying {
    playback: Entity<PlaybackEntity>,
    covers: Entity<CoverCache>,
    library: Entity<LibraryEntity>,
    shell: Entity<ShellEntity>,
    /// 这一栏正在画的两张封面（当前曲 + 下一首），用来判断缓存的 notify 到底有没有改变画面。
    shown: [Option<Arc<RenderImage>>; 2],
    _track_sub: Subscription,
    _cover_sub: Subscription,
    _library_sub: Subscription,
    _shell_sub: Subscription,
}

impl NowPlaying {
    pub fn new(
        playback: Entity<PlaybackEntity>,
        covers: Entity<CoverCache>,
        library: Entity<LibraryEntity>,
        shell: Entity<ShellEntity>,
        cx: &mut Context<Self>,
    ) -> Self {
        // 用 `subscribe` 而不是 `observe`：`PlaybackEntity` 播放期间每 100ms 因进度 notify 一次，
        // 而 gpui 的 `notify` 是全树失效；逐帧进度归 `SeekBar` / `VolumeControl`。
        let track_sub = cx.subscribe(&playback, |this, _, event: &PlaybackEvent, cx| {
            match event {
                PlaybackEvent::TrackChanged => {
                    this.refresh_covers(cx);
                    cx.notify();
                }
                // 状态变化也要跟：加载失败走的是 `Error → StateChanged`，只听换曲会让错误提示拖到切下一首才出现。
                PlaybackEvent::StateChanged => cx.notify(),
                // 设备列表只有底栏的设备菜单关心
                PlaybackEvent::Finished
                | PlaybackEvent::VolumeChanged
                | PlaybackEvent::DevicesChanged
                | PlaybackEvent::OutputModeChanged => {}
            }
        });

        // 封面解码完成时缓存会 notify。只在这一栏要画的那两张真的换了才重绘：
        // 否则滚动曲库时每张落地封面都拖着整栏重排一次。
        let cover_sub = cx.observe(&covers, |this, _, cx| {
            this.refresh_covers(cx);
        });

        // 关注状态、艺术家统计、下一首都来自曲库；曲库变了要重画。
        let library_sub = cx.observe(&library, |_, _, cx| cx.notify());
        // 浮层模式下右上角多一枚关闭键；面板实体不一定随根视图重绘，自己订阅外壳。
        let shell_sub = cx.observe(&shell, |_, _, cx| cx.notify());

        let mut this = Self {
            playback,
            covers,
            library,
            shell,
            shown: [None, None],
            _track_sub: track_sub,
            _cover_sub: cover_sub,
            _library_sub: library_sub,
            _shell_sub: shell_sub,
        };
        this.refresh_covers(cx);
        this
    }

    /// 缓存里任何一张图解码完都会 notify，这里只回答「这一栏要画的两张封面换了没有」。
    /// 发请求交给渲染路径的 [`crate::common::cover::cover_state`]，它本来就管三态。
    fn refresh_covers(&mut self, cx: &mut Context<Self>) {
        let current = self.playback.read(cx).current().map(|track| track.id);
        let next = App::update_entity(cx, &self.library, |library, _| {
            library.upcoming_ids(2).into_iter().nth(1)
        });
        let fresh = [
            current.and_then(|id| self.covers.read(cx).ready(id, COVER_SIZE)),
            next.and_then(|id| self.covers.read(cx).ready(id, UP_NEXT_COVER)),
        ];

        let changed = fresh
            .iter()
            .zip(&self.shown)
            .any(|(new, old)| match (new, old) {
                (Some(new), Some(old)) => !Arc::ptr_eq(new, old),
                (new, old) => new.is_some() != old.is_some(),
            });
        if changed {
            self.shown = fresh;
            cx.notify();
        }
    }

    /// 从曲库派生的三样：当前曲的艺术家名、该艺术家的分组统计、队列里的下一首。
    /// 全部读缓存 / 只取两首，适合每帧调用。
    fn derived(&self, track: Option<&Track>, cx: &mut App) -> Derived {
        App::update_entity(cx, &self.library, |library, _| {
            let artist = track
                .and_then(|t| t.artist.as_deref())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned);

            // 统计与关注都认拆分后的第一位：艺术家分组里没有「甲/乙」这一整串，
            // 拿整串去查会让这一栏的统计和关注按钮一起消失。显示的艺术家名仍是标签原文。
            let primary = artist
                .as_deref()
                .map(|name| artist_key_str(Some(name)))
                .filter(|name| *name != UNKNOWN_ARTIST);

            let stats = primary.and_then(|name| {
                library
                    .artists()
                    .iter()
                    .find(|group| group.name == name)
                    .map(|group| (group.track_count(), group.album_count()))
            });

            let following = primary.map(|name| library.is_following(name));
            // `upcoming(2)` 的第一首是当前曲（光标位），第二首才是真正的「下一首」。
            let next = library.upcoming(2).into_iter().nth(1);

            Derived {
                artist,
                stats,
                following,
                next,
            }
        })
    }
}

struct Derived {
    artist: Option<String>,
    stats: Option<(usize, usize)>,
    following: Option<bool>,
    next: Option<Track>,
}

impl Render for NowPlaying {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = self.shell.read(cx).right_panel_overlays();
        let (track, duration, error) = {
            let playback = self.playback.read(cx);
            (
                playback.current().cloned(),
                playback.duration(),
                playback.error().map(ToOwned::to_owned),
            )
        };
        let Derived {
            artist,
            stats,
            following,
            next,
        } = self.derived(track.as_ref(), cx);

        let canvas = div()
            .flex_none()
            .size(px(COVER_SIZE))
            .rounded(px(theme::RADIUS_LG))
            .overflow_hidden()
            .bg(theme::elevated())
            .flex()
            .items_center()
            .justify_center();

        let canvas = match track.as_ref() {
            Some(track) => {
                let state = crate::common::cover::cover_state(
                    &self.covers,
                    &self.library,
                    track.id,
                    COVER_SIZE,
                    cx,
                );
                match state {
                    crate::common::cover::Cover::Ready(image) => canvas.child(
                        img(ImageSource::Render(image))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    ),
                    crate::common::cover::Cover::Pending => {
                        canvas.child(crate::common::cover::skeleton(COVER_SIZE, theme::RADIUS_LG))
                    }
                    crate::common::cover::Cover::Missing => {
                        canvas.child(placeholder(track.title.chars().next()))
                    }
                }
            }
            None => canvas.child(placeholder(None)),
        };

        let (title, subtitle) = match track.as_ref() {
            Some(track) => (track.title.clone(), track.subtitle()),
            None => ("未在播放".to_owned(), String::new()),
        };
        let has_subtitle = !subtitle.is_empty();

        // 宽度撑满才能截断长标题。
        let mut hero = div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(theme::GAP_LG))
            .child(canvas)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(theme::GAP_XS))
                    .w_full()
                    .child(
                        div()
                            .id("np-title")
                            .w_full()
                            .text_size(px(theme::FONT_TITLE))
                            .font_weight(theme::WEIGHT_TITLE)
                            .text_color(theme::text())
                            .truncate()
                            .tooltip(crate::common::tooltip::builder(title.clone()))
                            .child(title),
                    )
                    .child(
                        div()
                            .id("np-subtitle")
                            .w_full()
                            .text_size(px(theme::FONT_CAPTION))
                            .text_color(theme::text_muted())
                            .truncate()
                            .when(has_subtitle, |line| {
                                line.tooltip(crate::common::tooltip::builder(subtitle.clone()))
                            })
                            .child(subtitle),
                    ),
            );

        if track.as_ref().is_some_and(|t| t.duration.is_some()) {
            hero = hero.child(
                div()
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(theme::text_muted())
                    .child(format!("时长 {}", format_duration(duration))),
            );
        }

        if let Some(error) = error {
            hero = hero.child(
                div()
                    .w(px(COVER_SIZE))
                    .rounded(px(theme::RADIUS_MD))
                    .bg(theme::danger_soft())
                    .p(px(theme::GAP_SM))
                    .text_size(px(theme::FONT_CAPTION))
                    .text_color(theme::danger())
                    .child(error),
            );
        }

        let mut body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::GAP_LG))
            .child(hero);

        if let Some(artist) = artist {
            body = body.child(about_artist(
                artist,
                stats,
                following.unwrap_or(false),
                self.library.clone(),
            ));
        }

        if let Some(track) = track.as_ref() {
            body = body.child(credits(track));
        }

        if let Some(next) = next {
            body = body.child(up_next(&next, &self.covers, &self.library, cx));
        }

        div()
            .flex_none()
            .relative()
            .w(px(theme::RIGHT_PANEL_W))
            // 只写 `w()` 压不住内容：flex 默认 `min-width: auto`，长歌名会把面板顶宽。`min_w` + `max_w` 钉死。
            .min_w(px(theme::RIGHT_PANEL_W))
            .max_w(px(theme::RIGHT_PANEL_W))
            .overflow_hidden()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .child(
                div()
                    .id("now-playing-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(theme::GAP_XL))
                    .child(body),
            )
            // 浮层盖在主区上时没有自己的标题栏，关闭键浮在封面外的右上角。
            .when(overlay, |panel| {
                panel.child(
                    div()
                        .absolute()
                        .top(px(theme::GAP_SM))
                        .right(px(theme::GAP_SM))
                        .child(panel_close_button("np-close", &self.shell)),
                )
            })
    }
}

fn card() -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .rounded(px(theme::RADIUS_CARD))
        .bg(theme::surface())
        .p(px(theme::GAP_MD))
}

/// 「关于艺术家」卡：底色用 [`theme::hero_tint`] 取色，其上文字一律白色以保 AA。
fn about_artist(
    name: String,
    stats: Option<(usize, usize)>,
    following: bool,
    library: Entity<LibraryEntity>,
) -> AnyElement {
    let tint = theme::hero_tint(Some(name.as_str()));
    let stats_text = match stats {
        Some((tracks, albums)) => {
            format!("{} 首 · {} 张专辑", thousands(tracks), thousands(albums))
        }
        None => String::new(),
    };

    let follow = crate::common::button::toggle_chip(
        "np-follow",
        if following {
            Glyph::HeartFilled
        } else {
            Glyph::Heart
        },
        if following { "已关注" } else { "关注" },
        following,
    )
    .tooltip(crate::common::tooltip::builder(if following {
        format!("取消关注 {name}")
    } else {
        format!("关注 {name}")
    }))
    .on_click({
        let name = name.clone();
        move |_, _, cx| {
            library.update(cx, |library, cx| {
                library.toggle_follow(name.clone(), cx);
            });
        }
    });

    let mut card = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .rounded(px(theme::RADIUS_CARD))
        .bg(tint)
        .p(px(theme::GAP_MD))
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(theme::GAP_SM))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(theme::FONT_BODY))
                        .font_weight(theme::WEIGHT_SUBTITLE)
                        .text_color(theme::text())
                        .child(name),
                )
                .child(follow),
        );

    if !stats_text.is_empty() {
        card = card.child(
            div()
                .text_size(px(theme::FONT_CAPTION))
                // 白字降到 0.85 不透明度作次要信息；底色是中深色，仍在 AA 之上。
                .text_color(theme::text().opacity(0.85))
                .child(stats_text),
        );
    }

    card.into_any_element()
}

/// 「制作人员」：曲目标签里只有艺术家 / 专辑 / 格式，没有词曲、制作人，只列可得的字段。
fn credits(track: &Track) -> AnyElement {
    let format = track.format().unwrap_or_else(|| "未知".to_owned());

    card()
        .child(crate::common::section::section_heading("制作人员"))
        .child(credit_row(
            "艺术家",
            track.artist.as_deref().unwrap_or("未知"),
        ))
        .child(credit_row("专辑", track.album.as_deref().unwrap_or("未知")))
        .child(credit_row("格式", format.as_str()))
        .into_any_element()
}

fn credit_row(label: &'static str, value: &str) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(theme::GAP_SM))
        .child(
            div()
                .flex_none()
                .w(px(48.))
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text_faint())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(theme::FONT_CAPTION))
                .text_color(theme::text())
                .child(value.to_owned()),
        )
}

/// 「下一首」预告：点一下直接跳到那一首。
fn up_next(
    next: &Track,
    covers: &Entity<CoverCache>,
    library: &Entity<LibraryEntity>,
    cx: &mut App,
) -> AnyElement {
    let id = next.id;
    let state = crate::common::cover::cover_state(covers, library, id, UP_NEXT_COVER, cx);

    let thumb = div()
        .flex_none()
        .size(px(UP_NEXT_COVER))
        .rounded(px(theme::RADIUS_LG))
        .overflow_hidden()
        .bg(theme::elevated())
        .flex()
        .items_center()
        .justify_center();

    let note = || {
        div()
            .text_size(px(theme::FONT_BODY))
            .text_color(theme::text_faint())
            .child("♪")
    };

    let thumb = match state {
        crate::common::cover::Cover::Ready(image) => thumb.child(
            img(ImageSource::Render(image))
                .size_full()
                .object_fit(ObjectFit::Cover),
        ),
        crate::common::cover::Cover::Missing => thumb.child(note()),
        crate::common::cover::Cover::Pending => thumb.child(crate::common::cover::skeleton(
            UP_NEXT_COVER,
            theme::RADIUS_SM,
        )),
    };

    let subtitle = next.subtitle();
    let activate = library.clone();

    div()
        .w_full()
        .child(crate::common::section::section_heading("下一首"))
        .child(
            div()
                .id("np-up-next")
                .w_full()
                .mt(px(theme::GAP_SM))
                .flex()
                .items_center()
                .gap(px(theme::GAP_MD))
                .cursor_pointer()
                .rounded(px(theme::RADIUS_MD))
                .child(thumb)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_size(px(theme::FONT_BODY))
                                .text_color(theme::text())
                                .child(next.title.clone()),
                        )
                        .when(!subtitle.is_empty(), |col| {
                            col.child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_size(px(theme::FONT_CAPTION))
                                    .text_color(theme::text_muted())
                                    .child(subtitle),
                            )
                        }),
                )
                .on_click(move |_, _, cx| {
                    activate.update(cx, |library, cx| {
                        library.activate(id, cx);
                    });
                }),
        )
        .into_any_element()
}

fn placeholder(initial: Option<char>) -> impl IntoElement {
    let text = match initial {
        Some(c) => c.to_uppercase().to_string(),
        None => "♪".to_owned(),
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(56.))
        .text_color(theme::text_faint())
        .child(text)
}
