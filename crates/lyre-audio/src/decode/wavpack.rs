//! `WavPack` 解码：symphonia 0.6 没有它的格式读取器，改用无 unsafe 实现的 `oxideav-wavpack`。
//! 音频块后挂的 `APEv2` 标签要按块链截掉：块迭代器走到标签处报 `InvalidMagic` 而不是当流结束。

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use oxideav_wavpack::{StreamIndex, decode_range};

use crate::error::{AudioError, Result};

/// 块头里 `ck_size` 的位置与长度：`wvpk` 之后紧跟 4 字节小端。
const MAGIC: &[u8; 4] = b"wvpk";
const HEADER_LEN: usize = 8;

/// 按组解码：`StreamIndex` 只走块头、不解码，播放时按成员组一组一组解。
/// 整首解进内存（24bit 5.1）是每五分钟 500MB；独占后端拿 `open` 做格式协商，全解要白等一秒多。
pub struct WavPackSource {
    /// 整个文件读进来并按块链截掉尾部标签。压缩数据比解出的 PCM 小得多，
    /// 而按组跳转要能随机访问，所以留整份。
    audio: Vec<u8>,
    /// 只走块头的索引：每组的帧数、声道数、字节范围都在里面。
    index: StreamIndex,
    /// 当前这一组解出的交错样本，取值是容器位宽的整数。
    frame: Vec<i32>,
    pos: usize,
    /// 下一组要从哪一帧开始解。
    next_frame: u64,
    channels: usize,
    rate: u32,
    /// 把样本归一化到 f32 的除数，由容器位宽决定。
    scale: f32,
}

impl WavPackSource {
    pub fn new(path: &Path) -> Result<Self> {
        let mut file = File::open(path).map_err(AudioError::FileOpen)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|err| AudioError::ReadPacket(err.to_string()))?;

        let audio_len = trim_trailing_tag(&bytes).len();
        bytes.truncate(audio_len);
        if !bytes.starts_with(MAGIC) {
            return Err(AudioError::Decode("不是 WavPack 文件".to_owned()));
        }

        // 采样率与位宽在第一个音频块的块头里，只解析这一块，不解码
        let (rate, bytes_per_sample) = {
            let block = oxideav_wavpack::first_audio_block(&bytes)
                .map_err(|err| AudioError::Decode(format!("{err:?}")))?
                .ok_or(AudioError::NoAudioData)?;
            let rate = block
                .sample_rate()
                .map_err(|err| AudioError::Decode(format!("{err:?}")))?
                .unwrap_or(44_100);
            (rate, block.header.flags.bytes_per_sample())
        };
        if rate == 0 {
            return Err(AudioError::Decode("WavPack 文件头没给采样率".to_owned()));
        }

        let index =
            StreamIndex::scan(&bytes).map_err(|err| AudioError::Decode(format!("{err:?}")))?;
        let channels = index.channels();
        if channels == 0 || index.set_count() == 0 {
            return Err(AudioError::NoAudioData);
        }

        // 16 位留在 16 位槽；24/32 位解码出来是 32 位槽的值（实测与 ffmpeg 的 s32le 逐样本相同）
        let scale = if bytes_per_sample <= 2 {
            32_768.0
        } else {
            2_147_483_648.0
        };

        let first_frame = index.first_frame();
        Ok(Self {
            audio: bytes,
            index,
            frame: Vec::new(),
            pos: 0,
            // 游标用**绝对**帧号：流的首帧未必是 0，索引给出的区间端点也是绝对的
            next_frame: first_frame,
            channels,
            rate,
            scale,
        })
    }

    pub const fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> u16 {
        u16::try_from(self.channels).unwrap_or(u16::MAX)
    }

    pub fn source_channels(&self) -> u16 {
        // 没有下混，输出声道数就是声源声道数
        self.channels()
    }

    /// 整首的帧数，取自索引里的帧区间。
    fn total_frames(&self) -> u64 {
        self.index
            .end_frame()
            .saturating_sub(self.index.first_frame())
    }

    pub fn total_duration(&self) -> Option<Duration> {
        let frames = self.total_frames();
        if frames == 0 {
            return None;
        }
        Some(Duration::from_secs_f64(
            frames as f64 / f64::from(self.rate),
        ))
    }

    /// 先把第一组解出来，让「文件根本解不开」在建链阶段就暴露，而不是静默放完。
    pub fn prefill(&mut self) -> Result<()> {
        if self.frame.is_empty() && !self.pump()? {
            return Err(AudioError::NoAudioData);
        }
        Ok(())
    }

    /// 只移动游标，真正的解码留到下一次取样本 —— 跳转因此是零成本的。
    #[allow(clippy::unnecessary_wraps)] // 签名对齐其它解码器，由 TrackDecoder 统一派发
    pub fn seek(&mut self, time: Duration) -> Result<()> {
        let offset = (time.as_secs_f64() * f64::from(self.rate)) as u64;
        let last = self.index.end_frame();
        self.next_frame = self.index.first_frame().saturating_add(offset).min(last);
        self.frame.clear();
        self.pos = 0;
        Ok(())
    }

    /// 解下一组。`false` 表示整首已经解完。
    fn pump(&mut self) -> Result<bool> {
        if self.pos < self.frame.len() {
            return Ok(true);
        }
        // 游标落在哪一组里，以及从游标到该组末尾还有多少帧
        let (start, count) = match self.index.set_for_frame(self.next_frame) {
            Some(set) => {
                let start = self.next_frame.max(u64::from(set.first_frame));
                (start, set.end_frame().saturating_sub(start))
            }
            None => return Ok(false),
        };
        if count == 0 {
            return Ok(false);
        }

        // 多声道拆成组成员块，`decode_range` 按组输出；逐块解再拼会破坏交错顺序
        let pcm = decode_range(&self.audio, &self.index, start, count)
            .map_err(|err| AudioError::Decode(format!("{err:?}")))?;
        let frames = pcm.len() / self.channels;
        if frames == 0 {
            return Ok(false);
        }

        self.next_frame = start + frames as u64;
        self.frame = pcm;
        self.pos = 0;
        Ok(true)
    }
}

impl Iterator for WavPackSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        while self.pos >= self.frame.len() {
            match self.pump() {
                Ok(true) => {}
                // 整首解完会得到 `false`
                Ok(false) => return None,
                Err(err) => {
                    log::warn!("WavPack 解码中断：{err}");
                    return None;
                }
            }
        }
        let sample = self.frame[self.pos];
        self.pos += 1;
        Some(sample as f32 / self.scale)
    }
}

/// 按 `ck_size` 走块链，返回只包含音频块的那一段。
///
/// 块 = `wvpk` + 4 字节 `ck_size` + `ck_size` 字节内容。走到魔数不是 `wvpk` 就停 ——
/// 那后面是 `APEv2` 标签之类的东西，解码器遇到它会报错而不是当流结束。
fn trim_trailing_tag(bytes: &[u8]) -> &[u8] {
    let mut offset = 0usize;
    while offset + HEADER_LEN <= bytes.len() {
        if bytes[offset..offset + 4] != *MAGIC {
            break;
        }
        let size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let next = offset
            .checked_add(HEADER_LEN)
            .and_then(|v| v.checked_add(size))
            .unwrap_or(bytes.len());
        if next > bytes.len() {
            break;
        }
        offset = next;
    }
    &bytes[..offset]
}

#[cfg(test)]
mod tests {
    use super::trim_trailing_tag;

    /// 造一个块：`wvpk` + 4 字节 `ck_size` + `ck_size` 字节内容。
    fn block(payload: usize) -> Vec<u8> {
        let mut bytes = b"wvpk".to_vec();
        bytes.extend_from_slice(&(payload as u32).to_le_bytes());
        bytes.extend(std::iter::repeat_n(0x11_u8, payload));
        bytes
    }

    /// 尾部挂 `APEv2` 标签是 `WavPack` 的常态，必须按块链截掉 ——
    /// 块迭代器走到标签处会报 `InvalidMagic` 而不是当流结束。
    #[test]
    fn a_trailing_apev2_tag_is_trimmed_off() {
        let mut bytes = block(10);
        let audio_len = bytes.len();
        bytes.extend_from_slice(b"APETAGEX");
        bytes.extend(std::iter::repeat_n(0x22_u8, 200));

        assert_eq!(trim_trailing_tag(&bytes), &bytes[..audio_len]);
    }

    #[test]
    fn every_audio_block_is_kept_in_order() {
        let first = block(8);
        let second = block(64);
        let mut bytes = first.clone();
        bytes.extend_from_slice(&second);
        let audio_len = bytes.len();
        bytes.extend_from_slice(b"APETAGEX");

        assert_eq!(trim_trailing_tag(&bytes).len(), audio_len);
    }

    /// 声明了长度却没有后续字节：这一块是残的，连同它一起截掉。
    #[test]
    fn a_truncated_block_is_not_counted() {
        let mut bytes = block(10);
        let audio_len = bytes.len();
        bytes.extend_from_slice(b"wvpk");
        bytes.extend_from_slice(&64u32.to_le_bytes());

        assert_eq!(trim_trailing_tag(&bytes), &bytes[..audio_len]);
    }

    /// 开头就不是 `wvpk`：什么都留不下，交给上层按「不是 WavPack」报错。
    #[test]
    fn a_non_wavpack_head_leaves_nothing() {
        assert!(trim_trailing_tag(b"RIFF....WAVEfmt ").is_empty());
        assert!(trim_trailing_tag(b"").is_empty());
        assert!(trim_trailing_tag(b"wvpk").is_empty(), "只有魔数、没有头");
    }

    /// `ck_size` 为 0 也是合法块，走完它应当继续看下一块而不是原地打转。
    #[test]
    fn a_zero_length_block_does_not_loop_forever() {
        let mut bytes = block(0);
        bytes.extend_from_slice(&block(4));
        let audio_len = bytes.len();
        bytes.extend_from_slice(b"APETAGEX");

        assert_eq!(trim_trailing_tag(&bytes).len(), audio_len);
    }
}
