//! 按**内容**（不是扩展名）挑解码器：`MAC ` → `ape-decoder`、`wvpk` → `oxideav-wavpack`、
//! `OggS` + `OpusHead` → `opus-pure`，ASF（WMA）拒绝，其余交给 symphonia。
//! 判扩展名不够：改了名的 APE 会被 symphonia 的 MPEG 探测当成 MP3 帧同步，放出满刻度噪声。

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use lyre_core::{SIGNATURE_PROBE_LEN, Signature, signature_of};

use crate::error::{AudioError, Result};

use super::ape::ApeSource;
use super::decoder::SymphoniaDecoder;
use super::opus::OpusSource;
use super::wavpack::WavPackSource;

/// 各解码器的大小差一个数量级（Opus 带解码状态缓冲）；每首只建一次、生命周期内只搬一次。
#[allow(clippy::large_enum_variant)]
pub enum TrackDecoder {
    Symphonia(SymphoniaDecoder),
    Ape(ApeSource),
    Opus(OpusSource),
    WavPack(WavPackSource),
}

impl TrackDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let head = probe_head(path)?;
        match signature_of(&head) {
            Some(Signature::Ape) => Ok(Self::Ape(ApeSource::new(path)?)),
            Some(Signature::WavPack) => Ok(Self::WavPack(WavPackSource::new(path)?)),
            Some(Signature::Opus) => Ok(Self::Opus(OpusSource::new(path)?)),
            Some(Signature::Wma) => Err(AudioError::Unsupported("WMA / ASF".to_owned())),
            None => Ok(Self::Symphonia(SymphoniaDecoder::new(path)?)),
        }
    }

    /// 解码器名，只用于日志。
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Symphonia(_) => "symphonia",
            Self::Ape(_) => "ape",
            Self::Opus(_) => "opus",
            Self::WavPack(_) => "wavpack",
        }
    }

    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Symphonia(d) => d.sample_rate(),
            Self::Ape(d) => d.sample_rate(),
            Self::Opus(d) => d.sample_rate(),
            Self::WavPack(d) => d.sample_rate(),
        }
    }

    /// 输出声道数（不做下混，等于声源声道数）。
    pub fn channels(&self) -> u16 {
        match self {
            Self::Symphonia(d) => d.channels(),
            Self::Ape(d) => d.channels(),
            Self::Opus(d) => d.channels(),
            Self::WavPack(d) => d.channels(),
        }
    }

    pub fn source_channels(&self) -> u16 {
        match self {
            Self::Symphonia(d) => d.source_channels(),
            Self::Ape(d) => d.source_channels(),
            Self::Opus(d) => d.source_channels(),
            Self::WavPack(d) => d.source_channels(),
        }
    }

    pub fn total_duration(&self) -> Option<Duration> {
        match self {
            Self::Symphonia(d) => d.total_duration(),
            Self::Ape(d) => d.total_duration(),
            Self::Opus(d) => d.total_duration(),
            Self::WavPack(d) => d.total_duration(),
        }
    }

    pub fn seek(&mut self, time: Duration) -> Result<()> {
        match self {
            Self::Symphonia(d) => d.seek(time),
            Self::Ape(d) => d.seek(time),
            Self::Opus(d) => d.seek(time),
            Self::WavPack(d) => d.seek(time),
        }
    }

    /// 起解码线程之前先把头一批样本解出来。
    /// 解不开必须在这里报错，吞掉的话这首一上来就被判成放完，列表循环像队列在无声快进。
    pub fn prefill(&mut self) -> Result<()> {
        match self {
            Self::Symphonia(d) => d.prefill(),
            Self::Ape(d) => d.prefill(),
            Self::Opus(d) => d.prefill(),
            Self::WavPack(d) => d.prefill(),
        }
    }
}

impl Iterator for TrackDecoder {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        match self {
            Self::Symphonia(d) => d.next(),
            Self::Ape(d) => d.next(),
            Self::Opus(d) => d.next(),
            Self::WavPack(d) => d.next(),
        }
    }
}

/// 读文件头的魔数。文件比探测长度短也能判（更长的签名不会匹配）。
fn probe_head(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(AudioError::FileOpen)?;
    let mut head = vec![0u8; SIGNATURE_PROBE_LEN];
    let read = file
        .read(&mut head)
        .map_err(|err| AudioError::ReadPacket(err.to_string()))?;
    head.truncate(read);
    Ok(head)
}
