//! 是前代 Mercuial Player 的独立出的部分代码构成的 Symphonia 解码器
//! 解码器本身并不依赖 GPUI ，可以被复用到其他项目

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use symphonia::core::audio::{Channels, GenericAudioBufferRef};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

use crate::error::{AudioError, Result};

use super::dsp::{self, LfeConfig};

/// 填到容量的这个比例就停，留一定余量。
const TARGET_FILL_NUMER: usize = 80;
const TARGET_FILL_DENOM: usize = 100;
/// 单次填充允许最多解码的包数。
const MAX_PACKETS_PER_FILL: u32 = 50;
/// 剩余播放时长低于此值就触发下一次填充。
const REFILL_THRESHOLD_MS: u32 = 100;

#[derive(Debug, PartialEq, Eq, Clone)]
enum DecoderState {
    Uninitialized,
    Ready,
    EndOfStream,
    Error(String),
}

// 计算缓冲容量。
const fn buffer_samples(sample_rate: u32, channels: u16, duration_ms: u32) -> usize {
    let want = (sample_rate as u64 * duration_ms as u64 * channels as u64) / 1000;
    if want < 4096 {
        4096
    } else if want > 4_000_000 {
        4_000_000
    } else {
        want as usize
    }
}

/// 解码器内部的一段交错 f32 缓冲。
struct AudioBuffer {
    samples: Vec<f32>,
    position: usize,
    capacity: usize,
    sample_rate: u32,
    channels: u16,
    refill_threshold_ms: u32,
}

impl AudioBuffer {
    fn new(capacity: usize, sample_rate: u32, channels: u16) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
            position: 0,
            capacity,
            sample_rate,
            channels,
            refill_threshold_ms: REFILL_THRESHOLD_MS,
        }
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.position >= self.samples.len()
    }

    #[inline]
    fn remaining(&self) -> usize {
        self.samples.len() - self.position
    }

    #[inline]
    fn next(&mut self) -> Option<f32> {
        if self.position < self.samples.len() {
            let sample = self.samples[self.position];
            self.position += 1;
            self.compact_if_needed();
            Some(sample)
        } else {
            None
        }
    }

    fn clear(&mut self) {
        self.samples.clear();
        self.position = 0;
    }

    fn append(&mut self, incoming: &[f32]) {
        self.samples.extend_from_slice(incoming);
    }

    fn needs_refill(&self) -> bool {
        (self.remaining() as u64 * 1000)
            < (u64::from(self.sample_rate)
                * u64::from(self.channels)
                * u64::from(self.refill_threshold_ms))
    }

    /// 及时回收已经消费掉的样本，否则一首长曲下来前端数组会缓慢涨到全曲长度。
    #[inline]
    fn compact_if_needed(&mut self) {
        if self.position >= 4096 && self.position * 2 >= self.samples.len() {
            let remaining = self.samples.len() - self.position;
            self.samples.copy_within(self.position.., 0);
            self.samples.truncate(remaining);
            self.position = 0;
        }
    }
}

/// 一个音轨的 symphonia 解码器：实现 [`Iterator`]，逐样本吐交错 f32，声道数与声源一致。
pub struct SymphoniaDecoder {
    path: PathBuf,
    sample_rate: u32,
    total_duration: Option<Duration>,
    state: DecoderState,
    buffer: AudioBuffer,
    scratch: Vec<f32>,
    decoder: Option<Box<dyn symphonia::core::codecs::audio::AudioDecoder>>,
    format: Option<Box<dyn FormatReader>>,
    track_id: Option<u32>,
    current_sample: u64,
    source_channels: u16,
}

impl SymphoniaDecoder {
    pub fn new(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(AudioError::FileOpen)?;
        let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            hint.with_extension(ext);
        }

        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|err| AudioError::Probe(err.to_string()))?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or(AudioError::NoAudioTrack)?;

        let params = track
            .codec_params
            .as_ref()
            .and_then(CodecParameters::audio)
            .ok_or(AudioError::MissingCodecParams)?;

        // 报 0 采样率的畸形头（`Some(0)`）与「没写采样率」同样退默认值。
        // 0 既让缓冲尺寸算成 0，又让下面的除法变成 inf。
        let stated_rate = params.sample_rate.filter(|rate| *rate > 0);
        let sample_rate = stated_rate.unwrap_or(44_100);
        // 声道数同样取自容器头，写 0 的畸形头退成单声道 —— 它会变成除数与帧长
        let source_channels = (params.channels.as_ref().map_or(2, Channels::count) as u16).max(1);

        // 时长只按容器声明的采样率算，若拿基本的 CD 级别的 44.1kHz 去推未知采样率的流，
        // 会得到一个看着合理、实际错误的秒数，使用 `try_from_secs_f64` 解决 inf / NaN。
        let total_duration = match (track.num_frames, stated_rate) {
            (Some(frames), Some(rate)) => {
                Duration::try_from_secs_f64(frames as f64 / f64::from(rate)).ok()
            }
            _ => None,
        };

        // 按声源的原始声道数解码。下混不在这里做 —— 目标声道数由输出设备决定，
        // 是播放链路上 `ChannelConvertSource` 的事。
        let buffer_duration_ms = if sample_rate <= 48_000 { 500 } else { 400 };
        let capacity = buffer_samples(sample_rate, source_channels, buffer_duration_ms);

        Ok(Self {
            path: path.to_path_buf(),
            sample_rate,
            total_duration,
            state: DecoderState::Uninitialized,
            buffer: AudioBuffer::new(capacity, sample_rate, source_channels),
            scratch: Vec::with_capacity(4096),
            decoder: None,
            format: None,
            track_id: None,
            current_sample: 0,
            source_channels,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 解码输出的声道数，与声源一致。
    pub const fn channels(&self) -> u16 {
        self.buffer.channels
    }

    pub fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    /// 源文件的原始声道数
    pub fn source_channels(&self) -> u16 {
        self.source_channels
    }

    /// 先解出一段数据再返回，避免起播瞬间欠载。
    /// 短文件本来就装不满缓冲，否则会被误判成坏文件。
    pub fn prefill(&mut self) -> Result<()> {
        if self.state == DecoderState::Uninitialized {
            self.initialize()?;
        }

        // 填充目标必须与 `fill()` 一致，否则这里永远等不到目标、只能空转到次数上限。
        let target = self.buffer.capacity * TARGET_FILL_NUMER / TARGET_FILL_DENOM;
        let mut attempts = 0;
        while self.buffer.remaining() < target
            && attempts < 200
            && self.state != DecoderState::EndOfStream
        {
            if let Err(err) = self.fill() {
                if self.state == DecoderState::EndOfStream {
                    break;
                }
                // 前几次失败先容忍，一直填不上才报错
                if attempts > 10 && self.buffer.remaining() < self.buffer.capacity * 50 / 100 {
                    return Err(AudioError::Prefill(format!(
                        "尝试 {attempts} 次后仍填不满：{err}"
                    )));
                }
            }
            attempts += 1;
        }

        if self.state == DecoderState::EndOfStream {
            return Ok(());
        }

        if self.buffer.remaining() < self.buffer.capacity * 50 / 100 {
            return Err(AudioError::Prefill(format!(
                "只填到 {}%",
                self.buffer.remaining() * 100 / self.buffer.capacity
            )));
        }
        Ok(())
    }

    /// 跳到指定位置。解码器还没初始化时只记录目标位置。
    pub fn seek(&mut self, time: Duration) -> Result<()> {
        let target_ts = (time.as_secs_f64() * f64::from(self.sample_rate)) as u64;

        if let (Some(format), Some(decoder), Some(track_id)) =
            (self.format.as_mut(), self.decoder.as_mut(), self.track_id)
        {
            let seek_to = SeekTo::Timestamp {
                ts: Timestamp::new(timestamp_of(target_ts)),
                track_id,
            };
            match format.seek(SeekMode::Accurate, seek_to) {
                Ok(_) => {
                    // 失败时保持原样，调用方可以继续播
                    self.current_sample = target_ts;
                    self.buffer.clear();
                    decoder.reset();
                    self.state = DecoderState::Ready;
                    Ok(())
                }
                Err(err) => Err(AudioError::Seek(format!("{err:?}"))),
            }
        } else {
            self.current_sample = target_ts;
            self.buffer.clear();
            self.state = DecoderState::Uninitialized;
            Ok(())
        }
    }

    fn initialize(&mut self) -> Result<()> {
        let file = File::open(&self.path).map_err(AudioError::FileOpen)?;
        let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

        let mut hint = Hint::new();
        if let Some(ext) = self.path.extension().and_then(|s| s.to_str()) {
            hint.with_extension(ext);
        }

        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|err| AudioError::Probe(err.to_string()))?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or(AudioError::NoAudioTrack)?;
        let track_id = track.id;
        let params = track
            .codec_params
            .as_ref()
            .and_then(CodecParameters::audio)
            .ok_or(AudioError::MissingCodecParams)?
            .clone();

        // 让编码器在头尾写入的补静音被切除，连播时曲间无缝
        let options = AudioDecoderOptions::default().gapless(true);
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &options)
            .map_err(|err| AudioError::DecoderCreate(err.to_string()))?;

        // 切歌到指定位置
        if self.current_sample > 0 {
            let seek_to = SeekTo::Timestamp {
                ts: Timestamp::new(timestamp_of(self.current_sample)),
                track_id,
            };
            if format.seek(SeekMode::Accurate, seek_to).is_ok() {
                decoder.reset();
            } else {
                self.current_sample = 0;
            }
        }

        self.format = Some(format);
        self.decoder = Some(decoder);
        self.track_id = Some(track_id);
        self.state = DecoderState::Ready;
        Ok(())
    }

    fn fill(&mut self) -> Result<()> {
        if self.state == DecoderState::Uninitialized {
            self.initialize()?;
        }
        if matches!(
            self.state,
            DecoderState::Error(_) | DecoderState::EndOfStream
        ) {
            return Ok(());
        }

        // 变量被破坏时也只是报错
        let (Some(format), Some(decoder), Some(track_id)) =
            (self.format.as_mut(), self.decoder.as_mut(), self.track_id)
        else {
            return Err(AudioError::NotInitialized);
        };

        let target_fill = self.buffer.capacity * TARGET_FILL_NUMER / TARGET_FILL_DENOM;
        let mut decoded_packets = 0;

        while self.buffer.remaining() < target_fill && decoded_packets < MAX_PACKETS_PER_FILL {
            let packet = match format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.state = DecoderState::EndOfStream;
                    break;
                }
                // 若是码流参数发发生变化，容器要求重建解码器
                Err(Error::ResetRequired) => {
                    decoder.reset();
                    continue;
                }
                Err(Error::IoError(ref err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.state = DecoderState::EndOfStream;
                    break;
                }
                Err(err) => {
                    let error = AudioError::ReadPacket(err.to_string());
                    self.state = DecoderState::Error(error.to_string());
                    return Err(error);
                }
            };

            if packet.track_id != track_id {
                continue;
            }

            match decoder.decode(&packet) {
                Ok(audio_buf) => {
                    self.scratch.clear();
                    let expected = self.buffer.channels;
                    Self::convert(&audio_buf, &mut self.scratch, expected);
                    self.buffer.append(&self.scratch);
                    decoded_packets += 1;
                }
                Err(Error::IoError(ref err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.state = DecoderState::EndOfStream;
                    break;
                }
                // 单个坏包跳过
                Err(Error::DecodeError(_)) => {}
                Err(err) => {
                    let error = AudioError::Decode(err.to_string());
                    self.state = DecoderState::Error(error.to_string());
                    return Err(error);
                }
            }
        }

        Ok(())
    }

    /// 交错成 f32，不做下混：目标声道数由输出设备决定。
    /// 解码包实际的声道数与容器声明不一致时（中途换声道配置的流）按实际布局对齐，
    /// 不处理的话后续所有声道都会整体错位。
    fn convert(audio_buf: &GenericAudioBufferRef<'_>, out: &mut Vec<f32>, expected: u16) {
        let actual = audio_buf.spec().channels().count();
        out.reserve(audio_buf.frames() * actual);
        audio_buf.copy_to_vec_interleaved(out);

        if actual == 0 || actual == expected as usize {
            return;
        }
        Self::align_channels(out, actual, expected as usize);
    }

    /// 把已经交错好的采样从 `actual` 声道就地重排成 `expected` 声道。
    fn align_channels(samples: &mut Vec<f32>, actual: usize, expected: usize) {
        let frames = samples.len() / actual;
        // 目标声道数可能比实际多（声明 6 声道、包里只有 2 声道），先留足位置再就地重写
        samples.resize(frames * expected.max(actual), 0.0);

        let mut aligned = vec![0.0f32; expected];
        let mut write = 0usize;
        for frame in 0..frames {
            {
                let source = &samples[frame * actual..(frame + 1) * actual];
                dsp::convert_frame(source, &mut aligned, None, LfeConfig::default());
            }
            samples[write..write + expected].copy_from_slice(&aligned);
            write += expected;
        }
        samples.truncate(write);
    }
}

impl Iterator for SymphoniaDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        // 填充失败但缓冲里还有继续使用
        if (self.buffer.is_empty() || self.buffer.needs_refill())
            && let Err(err) = self.fill()
            && self.buffer.is_empty()
        {
            log_error_once(&err.to_string());
            return None;
        }

        let sample = self.buffer.next();
        if sample.is_some() {
            self.current_sample += 1;
        }
        sample
    }
}

fn log_error_once(message: &str) {
    // 坏文件写日志
    log::warn!("[lyre-audio] 解码中止：{message}");
}

/// 采样序号转 `Timestamp` 需要的 `i64`。钳到上限写明意图。
fn timestamp_of(sample: u64) -> i64 {
    i64::try_from(sample).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{SymphoniaDecoder, buffer_samples};

    /// 容量必须夹在可用区间里：采样率直接取自文件头，坏数据可以是任意 u32。
    #[test]
    fn buffer_capacity_stays_bounded_for_absurd_header_rates() {
        assert_eq!(buffer_samples(u32::MAX, 2, 500), 4_000_000, "上限挡住 OOM");
        assert_eq!(buffer_samples(48_000, 2, 500), 48_000, "正常值不该被夹到");
        assert_eq!(buffer_samples(1, 1, 1), 4096, "再小也要留得下一格样本");
    }

    /// 声明 6 声道、包里只有 2 声道：目标比实际多，就地重写必须先扩容，
    /// 否则直接越界 panic（跑在解码线程上，会带走整个进程）。
    #[test]
    fn aligning_fewer_channels_than_declared_does_not_run_off_the_end() {
        let mut samples = vec![0.5, -0.5, 0.25, -0.25];
        SymphoniaDecoder::align_channels(&mut samples, 2, 6);

        assert_eq!(samples.len(), 12, "两帧六声道");
        assert_eq!(&samples[..2], &[0.5, -0.5], "前置两路留在原位");
        assert!(samples[2..].iter().all(|sample| *sample == 0.0));
    }

    /// 反向：声明 2 声道、包里是 6 声道，按 ITU 系数下混。
    #[test]
    fn aligning_more_channels_than_declared_downmixes() {
        let mut samples = Vec::new();
        for _ in 0..2 {
            samples.extend_from_slice(&[0.5, -0.4, 0.2, 0.9, 0.1, 0.2]);
        }
        SymphoniaDecoder::align_channels(&mut samples, 6, 2);

        let expected_left = (0.5 + 0.2 * 0.707 + 0.1 * 0.707) * 0.707;
        let expected_right = (-0.4 + 0.2 * 0.707 + 0.2 * 0.707) * 0.707;
        assert_eq!(samples.len(), 4);
        assert!(
            (samples[0] - expected_left).abs() < 1e-5,
            "L = {}",
            samples[0]
        );
        assert!(
            (samples[1] - expected_right).abs() < 1e-5,
            "R = {}",
            samples[1]
        );
        // 第二帧必须与第一帧一致，说明没有累积错位
        assert!((samples[2] - samples[0]).abs() < 1e-6);
        assert!((samples[3] - samples[1]).abs() < 1e-6);
    }
}
