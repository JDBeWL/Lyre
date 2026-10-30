//! 歌词侧文件：`.lrc` 与 `.ass` / `.ssa` 的解析与同目录查找。不读音频、不依赖 UI，可单测。
//! 双语是同一时间戳上的两行，第一条是原文。ASS 的角色靠 Style 名认：LDDC 把 `ts` 整块写在 `orig` 之前。

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 一个逐字区间。ASS 的 `{\kf}` 给的就是这个粒度；普通 LRC 行没有，`words` 留空。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricWord {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

/// 一行歌词。`end` 取下一行的开始（末行用文件里的结束时刻），只用来夹住逐字区间。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricLine {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
    /// 译文 / 罗马音。同刻多行时除第一条外都并进这里，用 `\n` 连接。
    pub translation: Option<String>,
    /// 逐字时间戳。空表示这一行没有逐字信息，界面整行一起点亮。
    pub words: Vec<LyricWord>,
}

impl LyricLine {
    /// 已唱到第几个字（含）。没有逐字信息时返回 `None`，界面据此退化成整行高亮。
    pub fn word_index(&self, position: Duration) -> Option<usize> {
        if self.words.is_empty() {
            return None;
        }
        match self
            .words
            .binary_search_by(|word| word.start.cmp(&position))
        {
            Ok(index) => Some(index),
            Err(0) => Some(0),
            Err(index) => Some(index - 1),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

impl Lyrics {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// 此刻该唱哪一行；首行之前返回 `None`。依赖 `lines` 按 `start` 严格升序（[`parse`] / [`parse_ass`] 保证）。
    pub fn active_index(&self, position: Duration) -> Option<usize> {
        match self
            .lines
            .binary_search_by(|line| line.start.cmp(&position))
        {
            Ok(index) => Some(index),
            Err(0) => None,
            Err(index) => Some(index - 1),
        }
    }
}

/// 一行在双语里的角色。声明顺序就是组内排序顺序：原文在前、译文在后。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    Original,
    /// LRC 没有角色信息，按文件里的先后定
    Unspecified,
    Translation,
}

struct Draft {
    start: Duration,
    /// 文件自己给的结束时刻（ASS 有，LRC 没有）
    end_hint: Option<Duration>,
    role: Role,
    text: String,
    /// 同一行内用 `\N` 分出来的第二段起就是译文
    inline_translation: Option<String>,
    words: Vec<LyricWord>,
}

/// 嗅探格式。认内容而不是只认扩展名：有人把 ASS 存成 `.lrc`。
pub fn parse_document(text: &str) -> Lyrics {
    let head = text.get(..4096).unwrap_or(text).to_ascii_lowercase();
    if head.contains("[script info]") || head.contains("[v4+ styles]") {
        return parse_ass(text);
    }
    parse(text)
}

/// 解析 LRC。支持一行多个时间标签、`[offset:]` 平移（文件级）与 `[ti:]/[ar:]/[al:]` 元信息。
pub fn parse(text: &str) -> Lyrics {
    let mut lyrics = Lyrics::default();
    // 正数让歌词提前出现（社区通行约定：时间戳减去偏移量）
    let mut offset_ms: i64 = 0;
    let mut drafts: Vec<Draft> = Vec::new();

    for line in text.lines() {
        let mut body = line.trim_start();
        let mut starts: Vec<Duration> = Vec::new();

        // 只吃掉行首连续的标签；遇到段名这类非时间戳的方括号就停下，但保留已收集的时间戳。
        while let Some(tag) = leading_tag(body) {
            if let Some(start) = parse_time(tag.content) {
                starts.push(start);
            } else if let Some((key, value)) = parse_meta(tag.content) {
                match key.as_str() {
                    "ti" => lyrics.title = Some(value.to_owned()),
                    "ar" => lyrics.artist = Some(value.to_owned()),
                    "al" => lyrics.album = Some(value.to_owned()),
                    "offset" => offset_ms = value.parse::<i64>().unwrap_or_default(),
                    _ => {}
                }
            } else {
                break;
            }
            body = tag.rest;
        }

        let text = body.trim();
        if text.is_empty() {
            continue;
        }
        for start in starts {
            drafts.push(Draft {
                start,
                end_hint: None,
                role: Role::Unspecified,
                text: text.to_owned(),
                inline_translation: None,
                words: Vec::new(),
            });
        }
    }

    // `[offset:]` 对整篇生效，哪怕它写在某行歌词之后
    if offset_ms != 0 {
        for draft in &mut drafts {
            let shifted = i64::try_from(draft.start.as_millis())
                .unwrap_or(i64::MAX)
                .saturating_sub(offset_ms);
            draft.start = Duration::from_millis(u64::try_from(shifted.max(0)).unwrap_or(u64::MAX));
        }
    }

    lyrics.lines = group(drafts);
    lyrics
}

/// 解析 ASS / SSA 歌词。取 `[Events]` 的 Dialogue 行，字段顺序由 `Format:` 声明决定。
pub fn parse_ass(text: &str) -> Lyrics {
    let mut lyrics = Lyrics::default();
    let mut section = Section::Other;
    let mut fields: Vec<String> = Vec::new();
    let mut drafts: Vec<Draft> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = match name.to_ascii_lowercase().as_str() {
                "script info" => Section::ScriptInfo,
                "v4 styles" | "v4+ styles" => Section::Styles,
                "events" => Section::Events,
                _ => Section::Other,
            };
            continue;
        }
        // 只在第一个冒号处切：正文里到处是冒号
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();

        match section {
            Section::ScriptInfo if key.eq_ignore_ascii_case("title") => {
                lyrics.title = Some(value.to_owned());
            }
            Section::Events if key.eq_ignore_ascii_case("format") => {
                fields = value
                    .split(',')
                    .map(|field| field.trim().to_owned())
                    .collect();
            }
            Section::Events if key.eq_ignore_ascii_case("dialogue") => {
                drafts.extend(dialogue(&fields, value));
            }
            // 其余一律不留：Style 表的角色是在 Dialogue 的 Style 名上认的，
            // ScriptInfo 的非 title 键与 Events 的非 format/dialogue 键同样丢弃。
            Section::ScriptInfo | Section::Styles | Section::Other | Section::Events => {}
        }
    }

    lyrics.lines = group(drafts);
    lyrics
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    ScriptInfo,
    Styles,
    Events,
    Other,
}

/// 一条 `Dialogue:` 的值。Text 恒为最后一个字段，所以按 `Format:` 的字段数限制切分次数。
fn dialogue(fields: &[String], value: &str) -> Option<Draft> {
    let names: Vec<&str> = if fields.is_empty() {
        vec![
            "Layer", "Start", "End", "Style", "Name", "MarginL", "MarginR", "MarginV", "Effect",
            "Text",
        ]
    } else {
        fields.iter().map(String::as_str).collect()
    };
    let values: Vec<&str> = value.splitn(names.len(), ',').collect();
    let field = |name: &str| {
        names
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(name))
            .and_then(|index| values.get(index))
            .map(|value| value.trim())
    };

    let start = ass_time(field("Start")?)?;
    let end = field("End").and_then(ass_time);
    let body = field("Text")?;
    if body.is_empty() {
        return None;
    }

    let (text, translation, words) = ass_text(body, start);
    if text.is_empty() && words.is_empty() {
        return None;
    }
    let text = if text.is_empty() {
        words.iter().map(|word| word.text.as_str()).collect()
    } else {
        text
    };

    Some(Draft {
        start,
        end_hint: end,
        role: style_role(field("Style").unwrap_or_default()),
        text,
        inline_translation: translation,
        words,
    })
}

/// Style 名 → 双语角色。短 ASCII 名只认全等（`contains` 会把 `Scripts` 误判成 `ts`），
/// 中文键才用包含，因为常见写法是带前缀的名字。
fn style_role(style: &str) -> Role {
    const ORIGINAL: &[&str] = &["orig", "original", "main", "source", "karaoke"];
    const TRANSLATION: &[&str] = &[
        "ts",
        "trans",
        "translation",
        "roma",
        "romaji",
        "lyricstrans",
    ];
    const TRANSLATION_CJK: &[&str] = &["译", "翻", "罗马"];
    const ORIGINAL_CJK: &[&str] = &["原文", "主"];

    let style = style.trim();
    let lowered = style.to_ascii_lowercase();
    if TRANSLATION.contains(&lowered.as_str())
        || TRANSLATION_CJK.iter().any(|key| style.contains(key))
    {
        return Role::Translation;
    }
    if ORIGINAL.contains(&lowered.as_str()) || ORIGINAL_CJK.iter().any(|key| style.contains(key)) {
        return Role::Original;
    }
    Role::Unspecified
}

/// `h:mm:ss.cc`（也容忍 `h:mm:ss:cc` 与省掉厘秒）→ 时长。
fn ass_time(value: &str) -> Option<Duration> {
    let mut parts = value.trim().split(':');
    let hours: u64 = parts.next()?.parse().ok()?;
    let minutes: u64 = parts.next()?.parse().ok()?;
    let seconds_field = parts.next()?;
    // 规范用 `.` 接厘秒，也有文件再挂一个 `:`
    let (seconds, centis) = match seconds_field.split_once('.') {
        Some((seconds, centis)) => (seconds.parse::<u64>().ok()?, centis.parse::<u64>().ok()?),
        None => match parts.next() {
            Some(centis) => (
                seconds_field.parse::<u64>().ok()?,
                centis.parse::<u64>().ok()?,
            ),
            None => (seconds_field.parse::<u64>().ok()?, 0),
        },
    };
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    Some(Duration::from_millis(
        hours
            .checked_mul(3_600_000)?
            .checked_add(minutes * 60_000 + seconds * 1000)?
            .checked_add(centis.checked_mul(10)?)?,
    ))
}

/// ASS 正文 →（原文, 译文, 逐字）。`\N` 之后的段落算译文；`{\k}` `{\kf}` `{\ko}`
/// 的值是时长（厘秒），按出现顺序累加成字区间。
fn ass_text(text: &str, start: Duration) -> (String, Option<String>, Vec<LyricWord>) {
    let mut cursor = start;
    let mut segments = Vec::new();
    for segment in split_line_breaks(text) {
        segments.push(karaoke_words(segment, &mut cursor));
    }
    let mut segments = segments
        .into_iter()
        .filter(|(plain, words)| !plain.is_empty() || !words.is_empty());

    let Some((body, words)) = segments.next() else {
        return (String::new(), None, Vec::new());
    };
    let translation = segments
        .map(|(plain, words)| {
            if plain.is_empty() {
                words
                    .iter()
                    .map(|word| word.text.as_str())
                    .collect::<String>()
            } else {
                plain
            }
        })
        .filter(|part: &String| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    // 逐字区间留给 `group` 统一夹取 —— 行尾要等下一行出现才知道
    (
        body,
        (!translation.is_empty()).then_some(translation),
        words,
    )
}

/// 按 ASS 的换行标记切段（`\N` 硬换行、`\n` 软换行）。大括号里的反斜杠不算。
fn split_line_breaks(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut begin = 0;
    let mut index = 0;
    let mut in_tag = false;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => in_tag = true,
            b'}' => in_tag = false,
            b'\\' if !in_tag && matches!(bytes.get(index + 1), Some(b'N' | b'n')) => {
                out.push(&text[begin..index]);
                begin = index + 2;
                index += 1;
            }
            _ => {}
        }
        index += 1;
    }
    out.push(&text[begin..]);
    out
}

/// 一段文本 →（去掉覆写标签的纯文本, 逐字区间）。`cursor` 是行内时间游标，跨段延续。
fn karaoke_words(segment: &str, cursor: &mut Duration) -> (String, Vec<LyricWord>) {
    let mut plain = String::new();
    let mut words: Vec<LyricWord> = Vec::new();
    let mut buf = String::new();
    let mut word_start = *cursor;
    let mut chars = segment.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' {
            let mut tag = String::new();
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
                tag.push(inner);
            }
            for part in tag.split('\\') {
                let Some(millis) = karaoke_duration(part) else {
                    continue;
                };
                if !buf.is_empty() {
                    words.push(LyricWord {
                        start: word_start,
                        end: *cursor,
                        text: std::mem::take(&mut buf),
                    });
                    word_start = *cursor;
                }
                *cursor = cursor.saturating_add(Duration::from_millis(millis));
            }
            continue;
        }
        plain.push(c);
        buf.push(c);
    }

    if !buf.is_empty() {
        words.push(LyricWord {
            start: word_start,
            end: *cursor,
            text: buf,
        });
    }
    (plain, words)
}

/// `\kf30` / `\k30` / `\ko+30` / `\k-30` → 毫秒。
fn karaoke_duration(tag: &str) -> Option<u64> {
    let tag = tag.trim_start();
    let rest = tag.strip_prefix(['k', 'K'])?;
    let rest = rest.strip_prefix(['f', 'F']).unwrap_or(rest);
    let rest = rest.strip_prefix(['o', 'O']).unwrap_or(rest);
    let digits = rest.trim_start_matches(['+', '-']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()?.checked_mul(10)
}

/// 逐字区间夹进本行 `[start, end)`，丢掉永远唱不到的尾格；剩下不足两格就退化成整行点亮。
fn fit_words(words: Vec<LyricWord>, start: Duration, end: Duration) -> Vec<LyricWord> {
    let mut last = start;
    let fitted: Vec<LyricWord> = words
        .into_iter()
        .filter(|word| !word.text.is_empty())
        .map(|word| {
            let begin = word.start.max(start).max(last);
            let finish = word.end.max(begin).min(end);
            last = finish;
            LyricWord {
                start: begin,
                end: finish,
                text: word.text,
            }
        })
        .take_while(|word| word.start < end)
        .collect();
    // 只有一格不算逐字 —— 逐字的意义就在于行内有先后
    if fitted.len() > 1 {
        fitted
    } else {
        Vec::default()
    }
}

/// 把散行配成「原文 + 译文」：先按 (开始时刻, 角色) 排序，同一时刻的原文顶到第一位，
/// 其余并进译文。末行留 5 秒尾巴，其余行的行尾就是下一行的行首。
fn group(mut drafts: Vec<Draft>) -> Vec<LyricLine> {
    drafts.sort_by_key(|draft| (draft.start, draft.role));

    let mut rows: Vec<LyricLine> = Vec::new();
    for draft in drafts {
        let same_row = rows
            .last()
            .is_some_and(|last: &LyricLine| last.start == draft.start);

        if !same_row {
            rows.push(LyricLine {
                start: draft.start,
                end: draft.end_hint.unwrap_or(draft.start),
                text: draft.text,
                translation: draft.inline_translation,
                words: draft.words,
            });
            continue;
        }

        // 上面那个分支已经保证 `rows` 非空：要么它刚 push 过一行，要么此前就非空。
        let Some(row) = rows.last_mut() else {
            continue;
        };
        push_translation(row, &draft.text);
        if let Some(part) = &draft.inline_translation {
            push_translation(row, part);
        }
        row.end = row.end.max(draft.end_hint.unwrap_or(draft.start));
    }

    for index in 0..rows.len() {
        let start = rows[index].start;
        let hinted = rows[index].end.max(start);
        let end = match rows.get(index + 1) {
            Some(next) => next.start.max(start),
            // 末行：文件给了 End 就以它为准（ASS 有），没给才留 5 秒尾巴（纯 LRC）
            None if hinted > start => hinted,
            None => start + Duration::from_secs(5),
        };
        rows[index].end = end;
        let words = std::mem::take(&mut rows[index].words);
        rows[index].words = fit_words(words, start, end);
    }
    rows
}

fn push_translation(row: &mut LyricLine, part: &str) {
    let part = part.trim();
    if part.is_empty() {
        return;
    }
    match &mut row.translation {
        Some(existing) => {
            existing.push('\n');
            existing.push_str(part);
        }
        None => row.translation = Some(part.to_owned()),
    }
}

/// 时间戳 → 播放位置。`[mm:ss]`、`[mm:ss.x]`、`[mm:ss.xx]`、`[mm:ss.xxx]`；
/// 小数分隔符允许 `.` `,` `:`（最后一种来自老 CD-G 文件的习惯）。
fn parse_time(tag: &str) -> Option<Duration> {
    let (minutes, rest) = tag.split_once(':')?;
    let minutes: u64 = digits(minutes)?.parse().ok()?;
    let (seconds, fraction) = match rest.split_once(['.', ',', ':']) {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (rest, ""),
    };
    let seconds: u64 = digits(seconds)?.parse().ok()?;
    if seconds > 59 {
        return None;
    }

    // 一位小数是十分之一秒、两位是百分之一秒、三位是毫秒；再多就截断
    let fraction_ms: u64 = match fraction.len() {
        0 => 0,
        1 => digits(fraction)?.parse::<u64>().ok()? * 100,
        2 => digits(fraction)?.parse::<u64>().ok()? * 10,
        _ => digits(fraction.get(..3).unwrap_or(fraction))?
            .parse::<u64>()
            .ok()?,
    };

    Some(Duration::from_millis(
        minutes
            .checked_mul(60_000)?
            .checked_add(seconds * 1000 + fraction_ms)?,
    ))
}

/// `[key:value]` 元信息。键限 2–6 个 ASCII 字母，否则 `[00:01.00]` 会被当成键 `00` 误吞。
/// 键统一转小写返回，好让 `[TI:]` 这类写法也认。
fn parse_meta(tag: &str) -> Option<(String, &str)> {
    let (key, value) = tag.split_once(':')?;
    let key = key.trim();
    if !(2..=6).contains(&key.len()) || !key.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some((key.to_ascii_lowercase(), value.trim()))
}

struct Tag<'a> {
    content: &'a str,
    rest: &'a str,
}

fn leading_tag(line: &str) -> Option<Tag<'_>> {
    let offset = line.get(1..)?.find(']')?;
    // 用 `get` 而不是切片：`[` 后紧跟多字节字符时字节索引不是字符边界，切片会 panic
    let content = line.get(1..offset + 1)?;
    Some(Tag {
        content,
        rest: line.get(offset + 2..)?,
    })
}

/// 只接受纯 ASCII 数字。非数字（`6a`、`--`）判为非法，而不是静默当成 0。
fn digits(value: &str) -> Option<&str> {
    (!value.is_empty() && value.chars().all(|c| c.is_ascii_digit())).then_some(value)
}

/// 侧文件后缀，按优先级排：已有 `.lrc` 的人不该因为旁边多了份 ASS 就换了显示。
const SIDECAR_SUFFIXES: &[&str] = &[".lrc", ".LRC", ".ass", ".ASS", ".ssa", ".SSA"];

/// 候选名：`song.lrc` 与 `song.mp3.lrc`（两种命名都在用），每种后缀各再试一次大写扩展名
/// （Windows 不区分大小写，其它平台区分）。
pub fn sidecar_candidates(audio: &Path) -> Vec<PathBuf> {
    let directory = audio.parent().unwrap_or(Path::new("."));
    let (stem, file_name) = match (audio.file_stem(), audio.file_name()) {
        (Some(stem), Some(name)) => (stem.to_owned(), name.to_owned()),
        _ => return Vec::new(),
    };

    let mut out = Vec::new();
    for base in [stem, file_name] {
        for suffix in SIDECAR_SUFFIXES {
            let mut name = base.clone();
            name.push(suffix);
            out.push(directory.join(name));
        }
    }
    out
}

/// 第一个真实存在的候选路径。
pub fn sidecar_path(audio: &Path) -> Option<PathBuf> {
    sidecar_candidates(audio)
        .into_iter()
        .find(|path| path.is_file())
}

/// 读侧文件并解析，供后台线程直接调用。
/// 没有侧文件、读不动、或一行歌词都没有时返回 `None`，由界面走「没有歌词」那一档。
pub fn load_for_audio(audio: &Path) -> Option<Lyrics> {
    let path = sidecar_path(audio)?;
    let text = decode_text(&std::fs::read(path).ok()?);
    let lyrics = parse_document(&text);
    (!lyrics.is_empty()).then_some(lyrics)
}

/// 侧文件没有强制编码，中文发行里 GBK/GB18030 仍常见。UTF-8 解不动就整段退到 GB18030，
/// 不逐行试 —— 逐行会让一首歌里混进两种编码。
fn decode_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    let (text, _, _) = encoding_rs::GB18030.decode(bytes);
    text.into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{
        Lyrics, load_for_audio, parse, parse_ass, parse_document, sidecar_candidates, sidecar_path,
    };

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn starts(lyrics: &Lyrics) -> Vec<u64> {
        lyrics
            .lines
            .iter()
            .map(|line| line.start.as_millis() as u64)
            .collect()
    }

    fn texts(lyrics: &Lyrics) -> Vec<&str> {
        lyrics.lines.iter().map(|line| line.text.as_str()).collect()
    }

    /// LDDC 真实样本的形状：`ts` 整块写在 `orig` 之前，同一句的两个 Style 时刻相同，`orig` 带 `{\kf}` 逐字。
    const ASS_SAMPLE: &str = "[Script Info]\n\
Title: 示例曲\n\
ScriptType: v4.00+\n\
\n\
[V4+ Styles]\n\
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: orig,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\
Style: ts,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\
Style: roma,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\
\n\
[Events]\n\
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,00:00:03.24,00:00:06.69,ts,,0,0,0,,甲译文\n\
Dialogue: 0,00:00:06.69,00:00:10.29,ts,,0,0,0,,乙译文\n\
Dialogue: 0,00:00:03.24,00:00:06.69,orig,,0,0,0,,{\\kf14}一{\\kf16}二{\\kf20}三{\\kf16}四{\\kf24}五{\\kf24}六{\\kf16}七{\\kf21}八{\\kf24}九{\\kf16}十{\\kf35}十一\n\
Dialogue: 0,00:00:06.69,00:00:10.29,orig,,0,0,0,,{\\kf39}字{\\kf21}符{\\kf41}串{\\kf17}甲{\\kf42}乙{\\kf24}丙\n\
Dialogue: 0,00:00:13.97,00:00:17.64,roma,,0,0,0,,romaji placeholder\n";

    #[test]
    fn ass_按_style_认原文与译文() {
        let lyrics = parse_ass(ASS_SAMPLE);
        assert_eq!(lyrics.title.as_deref(), Some("示例曲"));
        assert_eq!(lyrics.lines.len(), 3, "同刻的 ts/orig 要配成一行");
        assert_eq!(starts(&lyrics), [3_240, 6_690, 13_970]);

        let first = &lyrics.lines[0];
        // 译文行写在前面，靠角色排序也必须让原文当 text
        assert_eq!(first.text, "一二三四五六七八九十十一");
        assert_eq!(first.translation.as_deref(), Some("甲译文"));
    }

    #[test]
    fn ass_逐字区间按时长累加() {
        let lyrics = parse_ass(ASS_SAMPLE);
        let words = &lyrics.lines[0].words;
        assert_eq!(words.len(), 11);
        assert_eq!(words[0].text, "一");
        assert_eq!(words[0].start, ms(3_240));
        assert_eq!(words[0].end, ms(3_380), "\\kf14 = 140ms");
        assert_eq!(words[1].start, ms(3_380), "下一字接着上一字的末尾");
        assert_eq!(words[1].text, "二");
        // 11 格累计 = 3.24s + 2.26s
        assert_eq!(words.last().unwrap().end, ms(5_500));
    }

    #[test]
    fn ass_逐字越过行尾时夹住() {
        // 行只有 2 秒，两格各标 3 秒：第二格开始于 4s，已经在本行之外，只能丢掉。
        // 剩下不足两格就不再有「逐字」的意义，退化成整行点亮。
        let text =
            "[Events]\nDialogue: 0,00:00:01.00,00:00:03.00,orig,,0,0,0,,{\\kf300}甲{\\kf300}乙\n";
        let lyrics = parse_ass(text);
        assert!(
            lyrics.lines[0].words.is_empty(),
            "越界到只剩一格时不该保留逐字"
        );
        assert_eq!(lyrics.lines[0].text, "甲乙");
    }

    #[test]
    fn ass_末格被夹到行尾但仍保留逐字() {
        // 行 1s→5s，三格各 2 秒：第三格 [5s,7s] 整格越界被丢，前两格仍是有效的逐字
        let text = "[Events]\nDialogue: 0,00:00:01.00,00:00:05.00,orig,,0,0,0,,{\\kf200}甲{\\kf200}乙{\\kf200}丙\n";
        let lyrics = parse_ass(text);
        let words = &lyrics.lines[0].words;
        assert_eq!(words.len(), 2);
        assert_eq!((words[0].start, words[0].end), (ms(1_000), ms(3_000)));
        assert_eq!((words[1].start, words[1].end), (ms(3_000), ms(5_000)));
        assert_eq!(lyrics.lines[0].text, "甲乙丙", "正文仍含被丢掉的字");
    }

    /// 文件里的数字没有上界：小时位和 `{\kf}` 的厘秒都能写满 19 位，直接乘会溢出。
    #[test]
    fn ass_溢出时间戳被丢弃而不是溢出() {
        let text = "[Events]\nDialogue: 0,9999999999999999999:00:00.00,00:00:05.00,orig,,0,0,0,,{\\kf99999999999999999999}甲\n";
        assert!(parse_ass(text).lines.is_empty());
    }

    #[test]
    fn lrc_溢出分钟被丢弃而不是溢出() {
        assert!(parse("[99999999999999999999:00]甲").lines.is_empty());
    }

    #[test]
    fn ass_逐字定位与行定位一致() {
        let lyrics = parse_ass(ASS_SAMPLE);
        let line = &lyrics.lines[0];
        assert_eq!(line.word_index(ms(3_240)), Some(0));
        assert_eq!(line.word_index(ms(3_379)), Some(0));
        assert_eq!(line.word_index(ms(3_380)), Some(1));
        assert_eq!(line.word_index(ms(6_600)), Some(10));
        // 行首之前也要落在第一个字上，不能返回 None 让界面闪空
        assert_eq!(line.word_index(ms(1_000)), Some(0));
        // 没有逐字信息的行返回 None，界面退化成整行点亮
        assert_eq!(lyrics.lines[2].word_index(ms(14_000)), None);
    }

    #[test]
    fn ass_只取_events_且跳过注释与样式表() {
        let lyrics = parse_ass(ASS_SAMPLE);
        // Style 行里的 `Arial` 之类不能被当成歌词，Script Info 的 URL 注释也不能
        assert!(lyrics.lines.iter().all(|line| !line.text.contains("Arial")));
        assert!(
            lyrics
                .lines
                .iter()
                .all(|line| !line.text.contains("github"))
        );
    }

    #[test]
    fn ass_正文里的逗号不切字段() {
        let text = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,00:00:01.00,00:00:02.00,orig,,0,0,0,,你好，世界, 还有逗号\n";
        let lyrics = parse_ass(text);
        assert_eq!(lyrics.lines[0].text, "你好，世界, 还有逗号");
    }

    #[test]
    fn ass_换行标记的第二段算译文() {
        let text = "[Events]\nDialogue: 0,00:00:01.00,00:00:02.00,orig,,0,0,0,,原文第一行\\N译文第二行\\N第三行\n";
        let lyrics = parse_ass(text);
        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text, "原文第一行");
        assert_eq!(
            lyrics.lines[0].translation.as_deref(),
            Some("译文第二行\n第三行")
        );
    }

    #[test]
    fn ass_无逐字标记时退化成整行() {
        let text = "[Events]\nDialogue: 0,00:00:01.00,00:00:02.00,orig,,0,0,0,,{\\an8}只有排版标记的一行\n";
        let lyrics = parse_ass(text);
        assert!(lyrics.lines[0].words.is_empty());
        assert_eq!(lyrics.lines[0].text, "只有排版标记的一行");
    }

    #[test]
    fn ass_单格标记不算逐字() {
        let text =
            "[Events]\nDialogue: 0,00:00:01.00,00:00:09.00,orig,,0,0,0,,{\\kf800}作词：某人\n";
        let lyrics = parse_ass(text);
        assert!(
            lyrics.lines[0].words.is_empty(),
            "只有一格时逐字没有意义，应当整行显示"
        );
        assert_eq!(lyrics.lines[0].text, "作词：某人");
    }

    #[test]
    fn ass_坏行与未知段直接跳过() {
        let text = "[Events]\nDialogue: 0,00:00:01.00,00:00:02.00,orig,,0,0,0,,好行\n\
Dialogue: 0,99:99:99.99,00:00:02.00,orig,,0,0,0,,坏时刻\n\
Dialogue: 太短了\nComment: 0,00:00:05.00,00:00:06.00,orig,,0,0,0,,注释行\n\n";
        let lyrics = parse_ass(text);
        assert_eq!(texts(&lyrics), ["好行"]);
    }

    #[test]
    fn 嗅探能认出被改名成_lrc_的_ass() {
        let lyrics = parse_document(ASS_SAMPLE);
        assert_eq!(lyrics.lines.len(), 3);
        assert!(!lyrics.lines[0].words.is_empty());
    }

    #[test]
    fn 同刻两行配成原文加译文且保持升序() {
        let lyrics = parse("[00:01.00]中文\n[00:01.00]english\n[00:02.00]下一句\n");
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "中文");
        assert_eq!(lyrics.lines[0].translation.as_deref(), Some("english"));
        assert_eq!(starts(&lyrics), [1_000, 2_000]);
    }

    #[test]
    fn 三行同刻时后两行并进译文() {
        let lyrics = parse("[00:01.00]原文\n[00:01.00]译文\n[00:01.00]罗马音\n");
        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(
            lyrics.lines[0].translation.as_deref(),
            Some("译文\n罗马音"),
            "三种语言不能丢内容"
        );
    }

    #[test]
    fn 行尾等于下一行行首_末行留尾巴() {
        let lyrics = parse("[00:01.00]一\n[00:03.00]二\n");
        assert_eq!(lyrics.lines[0].end, ms(3_000));
        assert!(
            lyrics.lines[1].end > ms(3_000),
            "末行没有下一行可参照，要留一段尾巴"
        );
    }

    #[test]
    fn active_index_before_first_line_is_none() {
        let lyrics = parse("[00:10.00]第一句");
        assert_eq!(lyrics.active_index(ms(9_000)), None);
        assert_eq!(
            lyrics.active_index(ms(10_000)),
            Some(0),
            "正好落在时间戳上要算命中"
        );
        assert_eq!(lyrics.active_index(ms(600_000)), Some(0));
    }

    #[test]
    fn active_index_picks_the_last_line_at_or_before_position() {
        let lyrics = parse("[00:01.00]一\n[00:02.50]二\n[00:05.00]三");
        assert_eq!(lyrics.active_index(ms(2_499)), Some(0));
        assert_eq!(lyrics.active_index(ms(2_500)), Some(1));
        assert_eq!(lyrics.active_index(ms(4_999)), Some(1));
        assert_eq!(lyrics.active_index(ms(5_000)), Some(2));
    }

    #[test]
    fn parsed_lines_stay_sorted_for_binary_search() {
        let lyrics = parse("[00:20.00]后\n[00:05.00]前\n[01:00.00]末\n[00:10.00]中");
        assert_eq!(starts(&lyrics), [5_000, 10_000, 20_000, 60_000]);
        assert!(
            lyrics
                .lines
                .windows(2)
                .all(|pair| pair[0].start < pair[1].start)
        );
    }

    #[test]
    fn fractions_of_one_two_and_three_digits_are_all_milliseconds() {
        let lyrics = parse("[00:01.0]a\n[00:02.00]b\n[00:03.000]c\n[00:04.50]d\n[00:05.5]e");
        assert_eq!(starts(&lyrics), [1_000, 2_000, 3_000, 4_500, 5_500]);
    }

    #[test]
    fn timestamp_without_fraction_and_with_comma_or_colon() {
        let lyrics = parse("[01:02]甲\n[00:10,500]乙\n[00:20:50]丙");
        assert_eq!(starts(&lyrics), [10_500, 20_500, 62_000]);
    }

    #[test]
    fn one_line_can_carry_several_timestamps() {
        let lyrics = parse("[00:01.00][01:30.00]副歌重复");
        assert_eq!(starts(&lyrics), [1_000, 90_000]);
        assert_eq!(texts(&lyrics), ["副歌重复", "副歌重复"]);
    }

    #[test]
    fn identical_timestamp_and_text_collapses() {
        let lyrics = parse("[00:01.00][00:01.00]同一句");
        assert_eq!(lyrics.lines.len(), 1);
    }

    #[test]
    fn offset_shifts_every_timestamp_and_clamps_at_zero() {
        let earlier = parse("[offset:+1500]\n[00:02.00]提前一秒半\n[00:00.50]夹到零");
        assert_eq!(
            starts(&earlier),
            [0, 500],
            "减去偏移后再夹到零，顺序仍然升序"
        );
        assert_eq!(
            starts(&parse("[offset:-2000]\n[00:01.00]延后两秒")),
            [3_000]
        );
    }

    /// `[offset:]` 是文件级的：写在某行之后，也要平移它前面已经解析出来的行。
    #[test]
    fn offset_applies_to_the_whole_file_even_when_it_comes_late() {
        let lyrics = parse("[00:02.00]先来的那行\n[offset:+1000]\n[00:05.00]后来的那行");
        assert_eq!(starts(&lyrics), [1_000, 4_000]);
    }

    #[test]
    fn metadata_tags_are_read_and_not_turned_into_lines() {
        let lyrics = parse("[ti:曲目名]\n[ar:歌手]\n[AL:专辑]\n[by:打字的人]\n[00:01.00]正文");
        assert_eq!(lyrics.title.as_deref(), Some("曲目名"));
        assert_eq!(lyrics.artist.as_deref(), Some("歌手"));
        assert_eq!(lyrics.album.as_deref(), Some("专辑"));
        assert_eq!(texts(&lyrics), ["正文"]);
    }

    #[test]
    fn empty_and_untimed_lines_are_dropped() {
        let lyrics = parse("纯文字前言\n[00:01.00] 有时刻 \n[00:02.00]   \n[00:03.00]/\n");
        assert_eq!(texts(&lyrics), ["有时刻", "/"], "空白行不该占一个高亮位");
    }

    #[test]
    fn bracketed_section_label_stays_lyric_text() {
        let lyrics = parse("[00:01.00][副歌]唱这里");
        assert_eq!(texts(&lyrics), ["[副歌]唱这里"]);
    }

    #[test]
    fn malformed_timestamps_are_skipped_not_coerced() {
        let lyrics = parse("[00:6a.00]坏分钟\n[00:99.00]秒越界\n[ab:cd]怪键\n[00:01.00]好行");
        assert_eq!(texts(&lyrics), ["好行"]);
        assert!(lyrics.title.is_none());
    }

    #[test]
    fn a_file_that_is_not_lyrics_yields_no_lines() {
        assert!(parse("这根本不是歌词文件").is_empty());
        assert!(parse("").is_empty());
        assert!(parse("[]\n[[[\n[00:01.00").is_empty());
        assert!(parse_document("").is_empty());
        assert!(Lyrics::default().active_index(Duration::ZERO).is_none());
    }

    #[test]
    fn candidate_names_cover_both_spellings_and_all_suffixes() {
        let paths = sidecar_candidates(&PathBuf::from("D:/music/歌.mp3"));
        let names: Vec<String> = paths
            .iter()
            .take(6)
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["歌.lrc", "歌.LRC", "歌.ass", "歌.ASS", "歌.ssa", "歌.SSA"]
        );
        assert!(
            paths
                .iter()
                .any(|path| { path.file_name().is_some_and(|name| name == "歌.mp3.ass") })
        );
    }

    #[test]
    fn sidecar_lookup_prefers_lrc_over_ass() {
        let dir = temp_dir("prefer");
        let audio = dir.join("song.flac");
        std::fs::write(&audio, b"not audio at all").unwrap();
        assert_eq!(sidecar_path(&audio), None, "没有侧文件时不该凭空造一个路径");

        std::fs::write(dir.join("song.ass"), ASS_SAMPLE).unwrap();
        assert_eq!(sidecar_path(&audio), Some(dir.join("song.ass")));
        assert_eq!(load_for_audio(&audio).unwrap().lines.len(), 3);

        // 同时存在时 .lrc 优先：已有纯文本歌词的人不该被旁边的 ASS 换掉显示
        std::fs::write(dir.join("song.lrc"), "[00:01.00]纯文本歌词").unwrap();
        assert_eq!(sidecar_path(&audio), Some(dir.join("song.lrc")));
        assert_eq!(load_for_audio(&audio).unwrap().lines.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_for_audio_accepts_utf8_and_gb18030() {
        let dir = temp_dir("encoding");

        let utf8 = dir.join("utf8.mp3");
        std::fs::write(&utf8, b"x").unwrap();
        std::fs::write(dir.join("utf8.lrc"), "[00:01.00]城市漫游").unwrap();
        assert_eq!(load_for_audio(&utf8).unwrap().lines[0].text, "城市漫游");

        // 「测试」的 GB18030 字节不是合法 UTF-8，必须整段换编码
        let gbk = dir.join("gbk.mp3");
        std::fs::write(&gbk, b"x").unwrap();
        let mut gbk_bytes = b"[00:02.00]".to_vec();
        gbk_bytes.extend_from_slice(&[0xB2, 0xE2, 0xCA, 0xD4]);
        std::fs::write(dir.join("gbk.lrc"), gbk_bytes).unwrap();
        assert_eq!(load_for_audio(&gbk).unwrap().lines[0].text, "测试");

        // 有文件但一行歌词都没有：按「没有歌词」处理，别给界面一个空列表
        let silent = dir.join("silent.mp3");
        std::fs::write(&silent, b"x").unwrap();
        std::fs::write(dir.join("silent.lrc"), "只有文字没有时刻").unwrap();
        assert!(load_for_audio(&silent).is_none());
        assert!(load_for_audio(&dir.join("不存在.mp3")).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 每个用例一个独立子目录，避免并行跑测试时互相踩到同名文件。
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间早于 UNIX 纪元")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lyre-core-lyrics-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
