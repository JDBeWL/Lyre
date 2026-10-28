//! Opus / Ogg 解码。symphonia `all` 只注册 aac / adpcm / alac / flac / mpa / pcm / vorbis。
//! Ogg 认得出 Opus 流，建解码器时才报不支持，所以这里用 `opus-pure`：纯 Rust、自带 Ogg 封装。
//! Opus 恒在 48kHz 解码，头里的 `input_sample_rate` 只是原素材的说明，不影响解码。

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use opus_pure::{MAX_PACKET_SAMPLES, OggOpusReader, OpusMSDecoder};

use crate::error::{AudioError, Result};

/// Opus 解码器的工作采样率，固定 48k（RFC 6716）。
const RATE: u32 = 48_000;

/// 找最后一页时往回看的字节数。Ogg 页通常 4–8 KB，64 KB 足够覆盖最后一页。
const TAIL_SCAN: u64 = 64 * 1024;

/// 跳转时向前多解的预滚。RFC 7845 §4.6 建议至少 3840 个 48kHz 样本（80ms）。
const PREROLL_FRAMES: u64 = 3_840;

/// Ogg 页头固定长度：`OggS` + 版本 + 头类型 + granule + serial + seq + crc + 段数。
const PAGE_HEADER: usize = 27;

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
    reader: OggOpusReader<BufReader<OpusInput>>,
    path: PathBuf,
    decoder: OpusMSDecoder,
    channels: usize,
    /// 当前包解出的交错 f32。
    packet: Vec<f32>,
    pos: usize,
    /// 还要丢掉的交错样本数，用来消化 `pre_skip`。
    to_skip: usize,
    /// 头里的 `pre_skip`（每声道样本数），跳转时用来把 granule 换算成交付帧。
    pre_skip: u64,
    /// 整首的帧数（每声道样本数）。末包补齐到整包的填充不属于音频，交付满这个数必须停。
    limit: Option<u64>,
    /// 已经交付出去的帧数。
    handed_out: u64,
    total: Option<Duration>,
    /// 首次 seek 时扫描页头得到的索引；只用于跳转，不参与顺序播放。
    index: Option<PageIndex>,
    /// 测试用：最近一次 seek 实际解了多少个包，用来确认没有从头解到目标。
    #[cfg(test)]
    seek_packets: usize,
}

/// 测试用：从文件里实际读出的字节数，验收 seek 的读取量。
#[cfg(test)]
static READ_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 读取端：虚拟流 = 文件头 + 从 `target` 开始的后半段。
/// 让 `OggOpusReader` 正常解析 OpusHead/Tags 之后，直接落到目标页继续读。
struct OpusInput {
    file: File,
    len: u64,
    /// 虚拟流中「跳过去」的位置（通常是音频第一页的偏移）。
    split: u64,
    /// `split` 之后对应的真实文件偏移。
    target: u64,
    pos: u64,
}

impl OpusInput {
    fn plain(file: File) -> io::Result<Self> {
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            len,
            split: 0,
            target: 0,
            pos: 0,
        })
    }

    fn spliced(file: File, split: u64, target: u64) -> io::Result<Self> {
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            len,
            split,
            target,
            pos: 0,
        })
    }

    fn virtual_len(&self) -> u64 {
        self.split + self.len.saturating_sub(self.target)
    }
}

impl Read for OpusInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.virtual_len() {
            return Ok(0);
        }
        let (mapped, room) = if self.pos < self.split {
            (self.pos, self.split - self.pos)
        } else {
            let mapped = self.target + (self.pos - self.split);
            (mapped, self.len.saturating_sub(mapped))
        };
        let take = buf.len().min(usize::try_from(room).unwrap_or(usize::MAX));
        if take == 0 {
            return Ok(0);
        }
        self.file.seek(SeekFrom::Start(mapped))?;
        let read = self.file.read(&mut buf[..take])?;
        #[cfg(test)]
        READ_BYTES.fetch_add(read as u64, std::sync::atomic::Ordering::Relaxed);
        self.pos += read as u64;
        Ok(read)
    }
}

impl Seek for OpusInput {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let virtual_len = self.virtual_len();
        let next = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(delta) => i128::from(self.pos) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(virtual_len) + i128::from(delta),
        };
        if next < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start of Opus stream",
            ));
        }
        self.pos = next as u64;
        Ok(self.pos)
    }
}

/// 只走页头的索引。granule 是页尾已解码到的绝对帧号（含 `pre_skip`）。
#[derive(Clone, Copy)]
struct PageEntry {
    offset: u64,
    granule: i64,
}

struct PageIndex {
    /// 音频第一页的偏移，也是拼接读取器的切点。
    audio_start: u64,
    /// 只保留包边界页（非续页）且 granule 有效的页，按文件顺序递增。
    pages: Vec<PageEntry>,
}

impl OpusSource {
    pub fn new(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(AudioError::FileOpen)?;
        let input =
            OpusInput::plain(file).map_err(|err| AudioError::ReadPacket(err.to_string()))?;
        let reader = OggOpusReader::new(BufReader::new(input))
            .map_err(|err| AudioError::Decode(format!("打开 Ogg/Opus 失败：{err:?}")))?;
        let (channels, skip, family, gain) = {
            let head = reader.head();
            (
                usize::from(head.channel_count),
                u64::from(head.pre_skip),
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
            path: path.to_path_buf(),
            decoder,
            channels,
            packet: Vec::new(),
            pos: 0,
            to_skip: usize::try_from(skip).unwrap_or(usize::MAX) * channels,
            pre_skip: skip,
            limit,
            handed_out: 0,
            total: limit.map(|frames| Duration::from_secs_f64(frames as f64 / f64::from(RATE))),
            index: None,
            #[cfg(test)]
            seek_packets: 0,
        })
    }

    #[cfg(test)]
    fn index_page_count(&self) -> usize {
        self.index.as_ref().map_or(0, |index| index.pages.len())
    }

    #[cfg(test)]
    fn seek_packets(&self) -> usize {
        self.seek_packets
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

    /// 跳转：优先走 Ogg 页索引，只从目标前 80ms 的包边界开始解；
    /// 索引不可用时才退回从头顺序解码。
    pub fn seek(&mut self, time: Duration) -> Result<()> {
        self.decoder
            .reset_state()
            .map_err(|err| AudioError::Seek(format!("{err:?}")))?;
        self.packet.clear();
        self.pos = 0;
        self.to_skip = 0;
        #[cfg(test)]
        {
            self.seek_packets = 0;
        }

        let target = (time.as_secs_f64() * f64::from(RATE)) as u64;

        if let Some((audio_start, page)) = self.seek_page(target)
            && let Ok(reader) = self.reader_at(audio_start, page.offset)
        {
            self.reader = reader;
            self.to_skip = if page.offset == audio_start {
                (self.pre_skip as usize) * self.channels
            } else {
                0
            };
            return self.decode_from_page(page.granule, target);
        }

        self.seek_from_start(target)
    }

    /// 找目标前预滚位置之前、最近的包边界页。找不到索引时返回 `None`。
    fn seek_page(&mut self, target: u64) -> Option<(u64, PageEntry)> {
        if self.index.is_none() {
            match PageIndex::scan(&self.path, self.reader.serial()) {
                Ok(index) => self.index = Some(index),
                Err(err) => {
                    log::warn!("Opus 页索引建立失败，退回顺序 seek：{err}");
                    return None;
                }
            }
        }
        let index = self.index.as_ref()?;
        if index.pages.is_empty() {
            return None;
        }
        let point = target.saturating_sub(PREROLL_FRAMES);
        let idx = index.pages.partition_point(|page| {
            (page.granule.max(0) as u64).saturating_sub(self.pre_skip) <= point
        });
        // 没有页在预滚点之前结束：目标落在第一页里，回到从头顺序解码。
        if idx == 0 {
            return None;
        }
        Some((index.audio_start, index.pages[idx - 1]))
    }

    /// 用「文件头 + 目标页」的拼接源重建读取器：头部只读一次，中间的数据包整段跳过。
    fn reader_at(
        &self,
        audio_start: u64,
        target: u64,
    ) -> Result<OggOpusReader<BufReader<OpusInput>>> {
        let file = File::open(&self.path).map_err(AudioError::FileOpen)?;
        let input = OpusInput::spliced(file, audio_start, target)
            .map_err(|err| AudioError::ReadPacket(err.to_string()))?;
        OggOpusReader::new(BufReader::new(input))
            .map_err(|err| AudioError::Seek(format!("重建 Ogg 读取器失败：{err:?}")))
    }

    /// 从头顺序解码到目标；只在页索引不可用时使用。
    fn seek_from_start(&mut self, target: u64) -> Result<()> {
        let file = File::open(&self.path).map_err(AudioError::FileOpen)?;
        let input =
            OpusInput::plain(file).map_err(|err| AudioError::ReadPacket(err.to_string()))?;
        self.reader = OggOpusReader::new(BufReader::new(input))
            .map_err(|err| AudioError::Seek(format!("重建 Ogg 读取器失败：{err:?}")))?;
        self.to_skip = (self.pre_skip as usize) * self.channels;

        let channels = self.channels;
        let mut reached = 0u64;
        while reached < target {
            let Some(decoded) = self.decode_next()? else {
                break;
            };
            let frames = (decoded.pcm.len() / channels) as u64;
            if reached + frames > target {
                self.packet = decoded.pcm;
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

    /// 从索引页开始解码：该页 granule 给出页尾的绝对输出帧，之后按解码帧数推进到目标。
    fn decode_from_page(&mut self, page_granule: i64, target: u64) -> Result<()> {
        let channels = self.channels;
        let page_end = (page_granule.max(0) as u64).saturating_sub(self.pre_skip);
        let mut mapped = false;
        let mut after = 0u64;

        while let Some(decoded) = self.decode_next()? {
            #[cfg(test)]
            {
                self.seek_packets += 1;
            }
            let frames = (decoded.pcm.len() / channels) as u64;
            if frames == 0 {
                continue;
            }
            if !mapped {
                if decoded.page_granule == page_granule {
                    // 这一包完成在索引页上，整页都在目标之前
                    continue;
                }
                // 第一包跨到下一页：从它开始，输出位置就是索引页的页尾
                mapped = true;
            }
            let start = page_end.saturating_add(after);
            if start.saturating_add(frames) <= target {
                after = after.saturating_add(frames);
                continue;
            }
            let offset = usize::try_from(target.saturating_sub(start)).unwrap_or(0);
            self.packet = decoded.pcm;
            self.pos = offset * channels;
            self.handed_out = target;
            return Ok(());
        }

        self.handed_out = page_end.saturating_add(after);
        Ok(())
    }

    /// 解下一个包，返回交错 f32（已扣掉 `pre_skip`）与包所在页的 granule。`None` 表示流结束。
    fn decode_next(&mut self) -> Result<Option<DecodedPacket>> {
        loop {
            let packet = self
                .reader
                .read_packet()
                .map_err(|err| AudioError::Decode(format!("{err:?}")))?;
            let Some(packet) = packet else {
                return Ok(None);
            };
            let page_granule = packet.page_granule;

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
            return Ok(Some(DecodedPacket { pcm, page_granule }));
        }
    }

    fn pump(&mut self) -> Result<bool> {
        if self.pos < self.packet.len() {
            return Ok(true);
        }
        if let Some(decoded) = self.decode_next()? {
            self.packet = decoded.pcm;
            self.pos = 0;
            Ok(true)
        } else {
            self.packet.clear();
            Ok(false)
        }
    }
}

struct DecodedPacket {
    pcm: Vec<f32>,
    page_granule: i64,
}

impl PageIndex {
    /// 只读页头建立索引：不碰音频包，也不校验 CRC。
    fn scan(path: &Path, serial: u32) -> Result<Self> {
        let mut file = File::open(path).map_err(AudioError::FileOpen)?;
        let len = file
            .metadata()
            .map_err(|err| AudioError::ReadPacket(err.to_string()))?
            .len();

        let mut offset = 0u64;
        let mut matching = 0u32;
        let mut audio_start = None;
        let mut pages = Vec::new();

        while offset.saturating_add(PAGE_HEADER as u64) <= len {
            if file.seek(SeekFrom::Start(offset)).is_err() {
                break;
            }
            let mut header = [0u8; PAGE_HEADER];
            if file.read_exact(&mut header).is_err() || &header[..4] != b"OggS" {
                break;
            }
            let granule = i64::from_le_bytes([
                header[6], header[7], header[8], header[9], header[10], header[11], header[12],
                header[13],
            ]);
            let page_serial = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
            let continued = header[5] & 0x01 != 0;
            let segments_count = usize::from(header[26]);
            let mut segments = [0u8; 255];
            if file.read_exact(&mut segments[..segments_count]).is_err() {
                break;
            }
            let payload: u64 = segments[..segments_count]
                .iter()
                .map(|lace| u64::from(*lace))
                .sum();
            let page_len = PAGE_HEADER as u64 + segments_count as u64 + payload;
            let Some(next) = offset.checked_add(page_len) else {
                break;
            };
            if next > len {
                break;
            }

            if page_serial == serial {
                matching += 1;
                if matching == 2 {
                    // 首页是 OpusHead、第二页是 OpusTags；音频从第三页开始。
                    audio_start = Some(next);
                } else if matching > 2 && granule >= 0 && !continued {
                    pages.push(PageEntry { offset, granule });
                }
            }
            offset = next;
        }

        audio_start.map_or_else(
            || Err(AudioError::Seek("Ogg/Opus 里找不到音频页".to_owned())),
            |audio_start| Ok(Self { audio_start, pages }),
        )
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
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use opus_pure::{Application, OggOpusWriter, OpusEncoder, OpusHead};

    use super::{
        OpusSource, PageIndex, RATE, READ_BYTES, find, reorder_to_wav_order, rfind, total_frames,
    };

    fn take_read_bytes() -> u64 {
        READ_BYTES.swap(0, Ordering::Relaxed)
    }

    /// 用 opus-pure 自己的编码器生成一个真实 Ogg/Opus 文件（6 秒 440Hz 立体声）。
    fn fixture(path: &std::path::Path, seconds: u32) {
        let mut encoder = OpusEncoder::new(48_000, 2, Application::Audio).expect("建编码器");
        let head = OpusHead::for_encoder(&encoder, 48_000);
        let mut writer = OggOpusWriter::new(Vec::new(), head).expect("建 Ogg 写入器");
        let mut pcm = vec![0.0f32; 960 * 2];
        let mut out = vec![0u8; 4000];

        for packet in 0..seconds * 50 {
            for frame in 0..960 {
                let t = (packet * 960 + frame) as f32 / 48_000.0;
                let sample = (std::f32::consts::TAU * 440.0 * t).sin() * 0.3;
                let at = frame as usize * 2;
                pcm[at] = sample;
                pcm[at + 1] = sample;
            }
            let len = encoder.encode(&pcm, 960, &mut out).expect("编码测试 Opus");
            writer.write_packet(&out[..len]).expect("写 Opus 包");
        }

        let bytes = writer.finish().expect("收尾 Ogg");
        std::fs::write(path, bytes).expect("写测试文件");
    }

    fn assert_similar(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "样本数对不上");
        for (index, (a, b)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (a - b).abs() < 0.05,
                "第 {index} 个样本偏移过大：{a} vs {b}"
            );
        }
    }

    /// 索引里只收包边界页，granule 必须单调。
    #[test]
    fn the_page_index_lists_packet_boundaries_in_order() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let path = dir.path().join("tone.opus");
        fixture(&path, 6);

        let mut source = OpusSource::new(&path).expect("打开测试文件");
        source.seek(Duration::from_secs(3)).expect("建立索引");
        assert!(source.index_page_count() > 1, "长音频应当有多个可跳转页");

        let index = PageIndex::scan(&path, source.reader.serial()).expect("扫描索引");
        assert!(index.audio_start > 0);
        assert!(
            index
                .pages
                .windows(2)
                .all(|pair| pair[0].granule <= pair[1].granule)
        );
    }

    /// 跳到 0 秒必须回到第一帧；不能因为第一页还没结束就落到第二页。
    #[test]
    fn seeking_to_zero_starts_at_the_first_output_frame() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let path = dir.path().join("tone.opus");
        fixture(&path, 2);

        let mut sequential = OpusSource::new(&path).expect("打开");
        let expected: Vec<f32> = sequential.by_ref().take(960 * 2).collect();

        let mut indexed = OpusSource::new(&path).expect("打开");
        indexed.seek(Duration::ZERO).expect("seek 到开头");
        let actual: Vec<f32> = indexed.by_ref().take(960 * 2).collect();

        assert_similar(&actual, &expected);
    }

    /// 后段 seek 必须落回与顺序解码相同的音频，而且只解预滚附近的少量包。
    #[test]
    fn indexed_seek_matches_a_sequential_decode() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let path = dir.path().join("tone.opus");
        fixture(&path, 6);

        let target = Duration::from_millis(3_000);
        let skip = (target.as_secs_f64() * f64::from(RATE)) as usize * 2;
        let mut sequential = OpusSource::new(&path).expect("打开");
        let expected: Vec<f32> = sequential.by_ref().skip(skip).take(960 * 2).collect();

        let mut indexed = OpusSource::new(&path).expect("打开");
        indexed.seek(target).expect("索引 seek");
        let actual: Vec<f32> = indexed.by_ref().take(960 * 2).collect();

        assert_similar(&actual, &expected);
        assert!(
            indexed.seek_packets() < 100,
            "只该解预滚附近，实际解了 {} 个包（整段约 150 个）",
            indexed.seek_packets()
        );
    }

    /// 性能基准：29 秒处的 seek 不该把前 29 秒全部解码。
    #[test]
    #[ignore = "性能基准，手动运行"]
    fn seeking_near_the_end_skips_most_packets() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let path = dir.path().join("long.opus");
        fixture(&path, 30);

        let target = Duration::from_secs(29);
        let mut indexed = OpusSource::new(&path).expect("打开");
        let _ = take_read_bytes();
        let indexed_start = Instant::now();
        indexed.seek(target).expect("索引 seek");
        let indexed_time = indexed_start.elapsed();
        let indexed_bytes = take_read_bytes();
        let index_bytes = indexed.index.as_ref().map_or(0, |index| {
            index.pages.len() * std::mem::size_of::<super::PageEntry>()
        });
        let _ = indexed.next();

        let mut sequential = OpusSource::new(&path).expect("打开");
        let _ = take_read_bytes();
        let sequential_start = Instant::now();
        sequential.decoder.reset_state().expect("重置解码器");
        sequential
            .seek_from_start((target.as_secs_f64() * f64::from(RATE)) as u64)
            .expect("顺序 seek");
        let sequential_time = sequential_start.elapsed();
        let sequential_bytes = take_read_bytes();

        println!(
            "Opus 后段 seek：索引 {indexed_time:?} / {indexed_bytes} 字节 / {} 包，\
             顺序 {sequential_time:?} / {sequential_bytes} 字节；索引常驻 {index_bytes} 字节",
            indexed.seek_packets()
        );
        assert!(
            indexed_bytes < sequential_bytes,
            "索引 seek 读的字节应当少于从头解到目标（{indexed_bytes} vs {sequential_bytes}）"
        );
        assert!(
            indexed.seek_packets() < 200,
            "索引 seek 不该解到两千多个包里的绝大多数"
        );
    }

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
