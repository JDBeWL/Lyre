//! Opus / Ogg 解码。symphonia `all` 只注册 aac / adpcm / alac / flac / mpa / pcm / vorbis。
//! Ogg 认得出 Opus 流，建解码器时才报不支持，所以这里用 `opus-pure`：纯 Rust、自带 Ogg 封装。
//! Opus 恒在 48kHz 解码，头里的 `input_sample_rate` 只是原素材的说明，不影响解码。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use opus_pure::{MAX_PACKET_SAMPLES, OggOpusReader, OpusMSDecoder};

use crate::error::{AudioError, Result};

/// Opus 解码器的工作采样率，固定 48k（RFC 6716）。
const RATE: u32 = 48_000;

/// 找最后一页时往回看的字节数。Ogg 页通常 4–8 KB，64 KB 足够覆盖最后一页。
const TAIL_SCAN: u64 = 64 * 1024;

/// Vorbis 声道顺序 → 引擎假定的 WAV 顺序，`table[dst] = src`：两边只有 1 / 2 声道一致。
/// 多声道差在 LFE，Vorbis 排末路、WAV 排第四路；3 / 4 / 5 / 7 声道两边对不上，原样返回。
const WAV_ORDER: [(usize, &[usize]); 2] = [
    // 5.1：Vorbis FL FC FR RL RR LFE → WAV FL FR FC LFE BL BR
    (6, &[0, 2, 1, 5, 3, 4]),
    // 7.1：Vorbis FL FC FR RL RR SL SR LFE → WAV FL FR FC LFE BL BR SL SR
    (8, &[0, 2, 1, 7, 3, 4, 5, 6]),
];

/// 就地换序。规模不在表里就原样返回。
fn reorder_to_wav_order(pcm: &mut [f32], channels: usize) {
    let Some((_, table)) = WAV_ORDER.iter().find(|(count, _)| *count == channels) else {
        return;
    };
    let mut frame = [0.0f32; 8];
    for chunk in pcm.chunks_exact_mut(channels) {
        frame[..channels].copy_from_slice(chunk);
        for (dst, &src) in table.iter().enumerate() {
            chunk[dst] = frame[src];
        }
    }
}

pub struct OpusSource {
    reader: OggOpusReader<BufReader<File>>,
    decoder: OpusMSDecoder,
    channels: usize,
    /// 当前包解出的交错 f32。
    packet: Vec<f32>,
    pos: usize,
    /// 还要丢掉的交错样本数，用来消化 `pre_skip`。
    to_skip: usize,
    /// 整首的帧数（每声道样本数）。末包补齐到整包的填充不属于音频，交付满这个数必须停。
    limit: Option<u64>,
    /// 已经交付出去的帧数。
    handed_out: u64,
    total: Option<Duration>,
}

impl OpusSource {
    pub fn new(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(AudioError::FileOpen)?;
        let reader = OggOpusReader::new(BufReader::new(file))
            .map_err(|err| AudioError::Decode(format!("打开 Ogg/Opus 失败：{err:?}")))?;
        let (channels, skip, family, gain) = {
            let head = reader.head();
            (
                usize::from(head.channel_count),
                usize::from(head.pre_skip),
                head.mapping_family,
                i32::from(head.output_gain_q8),
            )
        };
        if channels == 0 {
            return Err(AudioError::Decode("Opus 头里声道数为 0".to_owned()));
        }

        // 5.1 之类不是一个包解 6 路，而是 Ogg 里若干条子流各解 1–2 声道再按映射表拼；
        // 普通 OpusDecoder 在声道数 > 2 时直接报错。
        let mut decoder = OpusMSDecoder::new(RATE.cast_signed(), channels, family)
            .map_err(|err| AudioError::Decode(format!("建 Opus 解码器失败：{err:?}")))?;
        // RFC 7845 §5.1：头里声明的增益必须落实，否则整首音量不对且没有别的症状。
        // multistream 是 N 条子流，每条都得设一遍。
        for stream in decoder.streams_mut() {
            stream.gain_q8 = gain;
        }

        let limit = total_frames(path);
        Ok(Self {
            reader,
            decoder,
            channels,
            packet: Vec::new(),
            pos: 0,
            to_skip: skip * channels,
            limit,
            handed_out: 0,
            total: limit.map(|frames| Duration::from_secs_f64(frames as f64 / f64::from(RATE))),
        })
    }

    /// 工作采样率是常数；签名与其它解码器一致，好让 [`super::TrackDecoder`] 统一派发。
    #[allow(clippy::unused_self)]
    pub const fn sample_rate(&self) -> u32 {
        RATE
    }

    pub fn channels(&self) -> u16 {
        u16::try_from(self.channels).unwrap_or(u16::MAX)
    }

    pub fn source_channels(&self) -> u16 {
        self.channels()
    }

    pub fn total_duration(&self) -> Option<Duration> {
        self.total
    }

    /// 先把第一个包解出来，让「容器认得、里面解不动」在建链阶段就暴露。
    pub fn prefill(&mut self) -> Result<()> {
        while self.pos >= self.packet.len() {
            if !self.pump()? {
                return Err(AudioError::NoAudioData);
            }
        }
        Ok(())
    }

    /// Opus 只能顺序解码，跳转就是回到头重新解到目标位置。
    pub fn seek(&mut self, time: Duration) -> Result<()> {
        self.reader
            .rewind()
            .map_err(|err| AudioError::Seek(format!("{err:?}")))?;
        self.decoder
            .reset_state()
            .map_err(|err| AudioError::Seek(format!("{err:?}")))?;

        let channels = self.channels;
        let skip = usize::from(self.reader.head().pre_skip);
        self.to_skip = skip * channels;
        self.packet.clear();
        self.pos = 0;

        let target = (time.as_secs_f64() * f64::from(RATE)) as u64;
        let mut reached = 0u64;
        while reached < target {
            let Some(batch) = self.decode_next()? else {
                break;
            };
            let frames = (batch.len() / channels) as u64;
            if reached + frames > target {
                self.packet = batch;
                self.pos = ((target - reached) as usize) * channels;
                // 交付计数要跟上跳进的位置，否则末包的截断上限会算错
                self.handed_out = target;
                return Ok(());
            }
            reached += frames;
            self.packet.clear();
        }
        self.handed_out = reached;
        Ok(())
    }

    /// 解下一个包，返回交错 f32（已扣掉 `pre_skip`）。`None` 表示流结束。
    fn decode_next(&mut self) -> Result<Option<Vec<f32>>> {
        loop {
            let packet = self
                .reader
                .read_packet()
                .map_err(|err| AudioError::Decode(format!("{err:?}")))?;
            let Some(packet) = packet else {
                return Ok(None);
            };

            let mut pcm = vec![0.0f32; MAX_PACKET_SAMPLES * self.channels];
            let frames = self
                .decoder
                .decode(&packet.data, MAX_PACKET_SAMPLES, &mut pcm)
                .map_err(|err| AudioError::Decode(format!("{err:?}")))?;
            pcm.truncate(frames * self.channels);

            if self.to_skip > 0 {
                let drop = self.to_skip.min(pcm.len());
                pcm.drain(..drop);
                self.to_skip -= drop;
                if pcm.is_empty() {
                    continue;
                }
            }
            reorder_to_wav_order(&mut pcm, self.channels);
            return Ok(Some(pcm));
        }
    }

    fn pump(&mut self) -> Result<bool> {
        if self.pos < self.packet.len() {
            return Ok(true);
        }
        if let Some(batch) = self.decode_next()? {
            self.packet = batch;
            self.pos = 0;
            Ok(true)
        } else {
            self.packet.clear();
            Ok(false)
        }
    }
}

impl Iterator for OpusSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.limit.is_some_and(|limit| self.handed_out >= limit) {
            return None;
        }
        match self.pump() {
            Ok(true) => {
                let sample = self.packet[self.pos];
                self.pos += 1;
                // 交错布局里每满一帧（`channels` 个样本）才计一帧
                if self.pos.is_multiple_of(self.channels) {
                    self.handed_out += 1;
                }
                Some(sample)
            }
            Ok(false) => None,
            Err(err) => {
                log::warn!("Opus 解码中断：{err}");
                None
            }
        }
    }
}

/// 整首的帧数（每声道样本数）：Ogg 只有最后一页的 granule 是全曲长度，
/// 播放器起播前就要有总时长，所以直接回扫文件尾而不是流式读完。
fn total_frames(path: &Path) -> Option<u64> {
    let (pre_skip, granule) = last_granule(path)?;
    // granule 从流的起点算起（含 pre_skip），交付的样本从 pre_skip 之后才数
    let frames = granule.checked_sub(i64::from(pre_skip))?;
    u64::try_from(frames).ok().filter(|frames| *frames > 0)
}

fn last_granule(path: &Path) -> Option<(u16, i64)> {
    let mut file = File::open(path).ok()?;
    let size = file.seek(SeekFrom::End(0)).ok()?;
    let start = size.saturating_sub(TAIL_SCAN);
    file.seek(SeekFrom::Start(start)).ok()?;

    let mut tail = Vec::with_capacity((size - start) as usize);
    file.take(size - start).read_to_end(&mut tail).ok()?;

    // 页头：0..4 "OggS"、5 头类型、6..14 granule position（i64 小端）
    let index = rfind(&tail, b"OggS")?;
    if index + 14 > tail.len() {
        return None;
    }
    let granule = i64::from_le_bytes(tail[index + 6..index + 14].try_into().ok()?);

    // pre_skip 在第一页的 OpusHead 里；这里只需要它，单独取一次
    let pre_skip = pre_skip_of(path).unwrap_or(0);
    Some((pre_skip, granule))
}

fn pre_skip_of(path: &Path) -> Option<u16> {
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; 64];
    let read = file.read(&mut head).ok()?;
    let head = &head[..read];
    let at = find(head, b"OpusHead")?;
    if at + 12 > head.len() {
        return None;
    }
    Some(u16::from_le_bytes([head[at + 10], head[at + 11]]))
}

/// 在字节串里找子串。`slice` 上的 `find` / `rfind` 还没稳定，这里自己走窗口。
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::{find, reorder_to_wav_order, rfind, total_frames};

    #[test]
    fn find_reports_the_first_hit_and_rfind_the_last() {
        let haystack = b"..OggS....OggS..";
        assert_eq!(find(haystack, b"OggS"), Some(2));
        assert_eq!(rfind(haystack, b"OggS"), Some(10));
        assert_eq!(find(haystack, b"nope"), None);
        // 针比草长：`windows` 给不出窗口，不该 panic
        assert_eq!(find(b"ab", b"abc"), None);
    }

    /// 造一个 Ogg 页：捕获串 + 版本 + 头类型 + 8 字节 granule + 页内容。
    fn page(granule: i64, body: &[u8]) -> Vec<u8> {
        let mut bytes = b"OggS".to_vec();
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&granule.to_le_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    /// 首页：`OpusHead` 落在偏移 28 —— 27 字节页头 + 1 字节段表。
    /// 头之后依次是版本、声道数、`pre_skip`。
    fn first_page(pre_skip: u16) -> Vec<u8> {
        let mut body = vec![0u8; 28 - 14];
        body.extend_from_slice(b"OpusHead");
        body.push(1); // 版本
        body.push(2); // 声道数
        body.extend_from_slice(&pre_skip.to_le_bytes());
        body.push(0); // 补够字节，`pre_skip` 的读取窗口要求至少 40 字节
        page(0, &body)
    }

    /// 帧数 = 最后一页 granule − `pre_skip`：只有末页的 granule 是全曲长度。
    #[test]
    fn duration_comes_from_the_last_page_minus_pre_skip() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let path = dir.path().join("tone.opus");

        let mut bytes = first_page(312);
        bytes.extend_from_slice(&page(312 + 48_000, &[0u8; 8]));
        std::fs::write(&path, &bytes).expect("写测试文件");

        assert_eq!(total_frames(&path), Some(48_000));
    }

    /// granule 还没追上 `pre_skip` 时没有有效样本，宁可不报也不报个负的。
    #[test]
    fn a_granule_below_pre_skip_yields_no_frames() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let path = dir.path().join("empty.opus");

        let mut bytes = first_page(312);
        bytes.extend_from_slice(&page(100, &[0u8; 8]));
        std::fs::write(&path, &bytes).expect("写测试文件");

        assert_eq!(total_frames(&path), None);
    }

    /// 没有 `OpusHead` 时 `pre_skip` 按 0 算，仍给得出帧数，不把整首长度一起丢掉。
    #[test]
    fn a_missing_pre_skip_falls_back_to_zero() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let path = dir.path().join("bare.opus");

        std::fs::write(&path, page(48_000, &[0u8; 16])).expect("写测试文件");

        assert_eq!(total_frames(&path), Some(48_000));
    }

    /// 6 声道：Vorbis 的 FL FC FR RL RR LFE 要变成 WAV 的 FL FR FC LFE BL BR。
    #[test]
    fn six_channels_move_lfe_to_the_fourth_slot() {
        let mut pcm: Vec<f32> = (1..=6).map(|v| v as f32).collect();
        reorder_to_wav_order(&mut pcm, 6);
        assert_eq!(pcm, vec![1.0, 3.0, 2.0, 6.0, 4.0, 5.0]);
    }

    #[test]
    fn eight_channels_move_lfe_to_the_fourth_slot() {
        let mut pcm: Vec<f32> = (1..=8).map(|v| v as f32).collect();
        reorder_to_wav_order(&mut pcm, 8);
        assert_eq!(pcm, vec![1.0, 3.0, 2.0, 8.0, 4.0, 5.0, 6.0, 7.0]);
    }

    /// 换序必须逐帧发生，不能把整段当成一条来搬。
    #[test]
    fn reordering_is_per_frame() {
        let mut pcm = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, // 第一帧
            7.0, 8.0, 9.0, 10.0, 11.0, 12.0, // 第二帧
        ];
        reorder_to_wav_order(&mut pcm, 6);
        assert_eq!(
            pcm,
            vec![
                1.0, 3.0, 2.0, 6.0, 4.0, 5.0, 7.0, 9.0, 8.0, 12.0, 10.0, 11.0
            ]
        );
    }

    /// 单声道 / 立体声两边一致，不该动。
    #[test]
    fn mono_and_stereo_are_left_alone() {
        let mut stereo = vec![1.0, 2.0, 3.0, 4.0];
        reorder_to_wav_order(&mut stereo, 2);
        assert_eq!(stereo, vec![1.0, 2.0, 3.0, 4.0]);

        let mut mono = vec![1.0, 2.0];
        reorder_to_wav_order(&mut mono, 1);
        assert_eq!(mono, vec![1.0, 2.0]);
    }
}
