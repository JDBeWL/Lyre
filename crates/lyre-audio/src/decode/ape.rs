//! APE（Monkey's Audio）解码。symphonia 的 `ape` feature 只注册标签读取器，没有音频解码器。
//! 它的 MPEG 探测会把 APE 的压缩数据误判成 MP3 帧同步并报「成功」，实际吐出满刻度噪声。
//! 这里用 `ape-decoder`：纯 Rust、无 FFI；按帧解码，`decode_all()` 会把整首装进内存。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use ape_decoder::ApeDecoder;

use crate::error::{AudioError, Result};

pub struct ApeSource {
    inner: ApeDecoder<BufReader<File>>,
    rate: u32,
    channels: u16,
    total_samples: u64,
    /// 下一个要解的帧号。
    next_frame: u32,
    /// 当前帧解出的交错 f32，空表示还没解或已解完。
    frame: Vec<f32>,
    pos: usize,
}

impl ApeSource {
    pub fn new(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(AudioError::FileOpen)?;
        let decoder = ApeDecoder::new(BufReader::new(file))
            .map_err(|err| AudioError::Decode(format!("打开 APE 失败：{err}")))?;
        let info = decoder.info().clone();

        if info.channels == 0 || info.sample_rate == 0 {
            return Err(AudioError::Decode(
                "APE 文件头缺少声道数或采样率".to_owned(),
            ));
        }

        Ok(Self {
            inner: decoder,
            rate: info.sample_rate,
            channels: info.channels,
            total_samples: info.total_samples,
            next_frame: 0,
            frame: Vec::new(),
            pos: 0,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn source_channels(&self) -> u16 {
        self.channels
    }

    pub fn total_duration(&self) -> Option<Duration> {
        if self.rate == 0 {
            return None;
        }
        Some(Duration::from_secs_f64(
            self.total_samples as f64 / f64::from(self.rate),
        ))
    }

    pub fn seek(&mut self, time: Duration) -> Result<()> {
        if self.rate == 0 {
            return Ok(());
        }
        let target = (time.as_secs_f64() * f64::from(self.rate)) as u64;
        let landed = self
            .inner
            .seek(target)
            .map_err(|err| AudioError::Seek(err.to_string()))?;

        self.frame.clear();
        self.pos = 0;
        self.decode_frame(landed.frame_index)?;
        // `skip_samples` 是每声道的样本数，交错布局里要乘声道数
        self.pos = landed.skip_samples as usize * self.channels as usize;
        Ok(())
    }

    /// 解一帧进来；已经是最后一帧之后就把 `frame` 清空表示结束。
    fn decode_frame(&mut self, index: u32) -> Result<()> {
        self.frame.clear();
        if index >= self.inner.info().total_frames {
            self.next_frame = index;
            return Ok(());
        }

        let pcm = self
            .inner
            .decode_frame(index)
            .map_err(|err| AudioError::Decode(err.to_string()))?;
        let info = self.inner.info().clone();
        self.frame = to_f32(&pcm, &info);
        self.next_frame = index + 1;
        Ok(())
    }

    /// 先把第一帧解出来，让「文件根本解不开」在建链阶段就暴露，而不是静默放完。
    pub fn prefill(&mut self) -> Result<()> {
        if self.frame.is_empty() && !self.pump()? {
            return Err(AudioError::NoAudioData);
        }
        Ok(())
    }

    /// 拉下一帧。`false` 表示整首已经解完。
    fn pump(&mut self) -> Result<bool> {
        if !self.frame.is_empty() {
            return Ok(true);
        }
        let index = self.next_frame;
        if index >= self.inner.info().total_frames {
            return Ok(false);
        }
        self.pos = 0;
        self.decode_frame(index)?;
        Ok(!self.frame.is_empty())
    }
}

impl Iterator for ApeSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        while self.pos >= self.frame.len() {
            match self.pump() {
                Ok(true) => {}
                // 整首解完得到 `false`；解码出错记一条日志后停在这里
                Ok(false) => return None,
                Err(err) => {
                    log::warn!("APE 解码中断：{err}");
                    return None;
                }
            }
        }
        let sample = self.frame[self.pos];
        self.pos += 1;
        Some(sample)
    }
}

/// 把 APE 解出的原始字节按容器规格转成 f32：位宽与是否浮点必须取自文件头，猜错就是直流偏移或削顶。
/// APE 有 8 / 16 / 24 / 32 位整数和 32 位浮点，8 位还分有符号与无符号。
fn to_f32(pcm: &[u8], info: &ape_decoder::ApeInfo) -> Vec<f32> {
    let bytes = usize::from(info.bytes_per_sample);
    if bytes == 0 || bytes > 4 {
        return Vec::new();
    }

    pcm.chunks_exact(bytes)
        .map(|chunk| match bytes {
            1 => {
                if info.is_signed_8bit {
                    f32::from(chunk[0].cast_signed()) / 128.0
                } else {
                    (f32::from(chunk[0]) - 128.0) / 128.0
                }
            }
            2 => f32::from(i16::from_le_bytes([chunk[0], chunk[1]])) / 32_768.0,
            3 => {
                let extended = i32::from(chunk[2].cast_signed()) << 16
                    | i32::from(chunk[1]) << 8
                    | i32::from(chunk[0]);
                extended as f32 / 8_388_608.0
            }
            _ => {
                if info.is_floating_point {
                    f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
                } else {
                    i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f32
                        / 2_147_483_648.0
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::to_f32;
    use ape_decoder::{ApeInfo, SourceFormat};

    /// 只有位宽、有无符号、是否浮点三项影响转换，其余字段填个合法值即可。
    fn info(bytes_per_sample: u16, signed_8bit: bool, floating_point: bool) -> ApeInfo {
        ApeInfo {
            version: 3_990,
            compression_level: 2_000,
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: bytes_per_sample * 8,
            total_samples: 0,
            total_frames: 0,
            blocks_per_frame: 0,
            final_frame_blocks: 0,
            duration_ms: 0,
            block_align: bytes_per_sample * 2,
            format_flags: 0,
            bytes_per_sample,
            average_bitrate_kbps: 0,
            decompressed_bitrate_kbps: 0,
            file_size_bytes: 0,
            is_big_endian: false,
            is_floating_point: floating_point,
            is_signed_8bit: signed_8bit,
            source_format: SourceFormat::Unknown,
        }
    }

    /// 16 位按 32768 归一：满量程负值是 -1.0，最大值略小于 1.0。
    #[test]
    fn sixteen_bit_maps_the_full_scale() {
        let out = to_f32(&[0x00, 0x80, 0xFF, 0x7F], &info(2, true, false));
        assert_eq!(out[0], -1.0);
        assert!((out[1] - 1.0).abs() < 1e-4, "实际 {}", out[1]);
    }

    /// 8 位有两种解释：有符号是补码，无符号是偏移二进制。猜错就是整体加了个直流。
    #[test]
    fn eight_bit_tells_signed_from_unsigned() {
        let signed = to_f32(&[0x80, 0x00, 0xFF], &info(1, true, false));
        assert_eq!(signed[0], -1.0, "有符号 0x80 是 -128");
        assert_eq!(signed[1], 0.0, "有符号 0x00 是 0");
        assert!((signed[2] + 0.007_812_5).abs() < 1e-6, "实际 {}", signed[2]);

        let unsigned = to_f32(&[0x80, 0x00, 0xFF], &info(1, false, false));
        assert_eq!(unsigned[0], 0.0, "无符号 0x80 是中点");
        assert_eq!(unsigned[1], -1.0, "无符号 0x00 是负满量程");
        assert!((unsigned[2] - 0.992_187_5).abs() < 1e-6);
    }

    /// 24 位是 3 字节小端、最高位在第 3 字节的符号位上，漏了符号扩展会把 -1.0 读成 +1.0。
    #[test]
    fn twenty_four_bit_sign_extends() {
        let out = to_f32(&[0x00, 0x00, 0x80, 0xFF, 0xFF, 0x7F], &info(3, true, false));
        assert_eq!(out[0], -1.0);
        assert!((out[1] - 1.0).abs() < 1e-6, "实际 {}", out[1]);
    }

    /// 32 位槽里可能是整数也可能是浮点，靠文件头的标志位分流，不能猜。
    #[test]
    fn thirty_two_bit_splits_integer_from_float() {
        let integer = to_f32(&[0x00, 0x00, 0x00, 0x80], &info(4, true, false));
        assert_eq!(integer[0], -1.0);

        let float = to_f32(&0.5f32.to_le_bytes(), &info(4, true, true));
        assert_eq!(float[0], 0.5);
    }

    /// 位宽为 0 或超出 4 字节时不输出，不按错误的除数归一。
    #[test]
    fn implausible_widths_produce_nothing() {
        assert!(to_f32(&[0x01, 0x02], &info(0, true, false)).is_empty());
        assert!(to_f32(&[0x01; 8], &info(5, true, false)).is_empty());
    }

    /// 末尾不足一个样本的字节要丢掉，不能拼成半个样本。
    #[test]
    fn a_partial_trailing_sample_is_dropped() {
        let out = to_f32(&[0x00, 0x80, 0x11, 0x22, 0x33], &info(2, true, false));
        assert_eq!(out.len(), 2);
    }
}
