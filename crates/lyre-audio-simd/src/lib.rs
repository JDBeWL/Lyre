//! 交错 f32 采样 → 声卡要的 PCM 字节，独占渲染链路上最费 CPU 的一段。
//! `F32` 走零拷贝视图，整数格式走 AVX2 / SSE2 内核，标量既兜底也是基准（差不得超过 1 LSB）。
//! 只有 SIMD 内核需要 `unsafe`：`[lints]` 在本 crate 局部放开，其余 crate 仍继承 forbid。

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86;

/// 独占模式协商出来的采样格式。位数与容器宽度一致（WASAPI 的 24-in-24 就是紧凑 3 字节）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PcmFormat {
    /// 32 位浮点。
    F32,
    /// 32 位整数。
    I32,
    /// 24 位整数，紧凑 3 字节。
    I24,
    /// 16 位整数。
    I16,
}

impl PcmFormat {
    /// 每个采样占几个字节，也是 WASAPI 的 block align 除以声道数。
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::I24 => 3,
            Self::I16 => 2,
        }
    }

    /// 一个满刻度采样（±1.0）对应的整数值。
    pub(crate) const fn full_scale(self) -> f32 {
        match self {
            Self::F32 => 1.0,
            Self::I32 => 2_147_483_648.0,
            Self::I24 => 8_388_608.0,
            Self::I16 => 32_768.0,
        }
    }
}

/// 把交错采样按目标格式写进 `out`：先清空，再写满。
pub fn write(samples: &[f32], format: PcmFormat, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(samples.len() * format.bytes_per_sample());

    match format {
        PcmFormat::F32 => scalar::f32(samples, out),
        PcmFormat::I32 => int32(samples, out),
        PcmFormat::I24 => scalar::i24(samples, out),
        PcmFormat::I16 => int16(samples, out),
    }
}

/// `F32` 的零拷贝视图：布局与小端平台上 WASAPI 要的交错 32 位浮点完全一致，整条转换链省掉。
pub fn f32_as_bytes(samples: &[f32]) -> &[u8] {
    let len = std::mem::size_of_val(samples);
    // SAFETY: `f32` 对齐不低于 `u8`，长度由切片长度乘以 4 得出不会越界；
    // 读成 `&[u8]` 对任何位模式都合法，也没有生命周期上的额外要求。
    unsafe { std::slice::from_raw_parts(samples.as_ptr().cast::<u8>(), len) }
}

fn int32(samples: &[f32], out: &mut Vec<u8>) {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if x86::avx2_available() {
            let head = x86::head_len(samples.len(), x86::avx2_lanes());
            if head > 0 {
                // SAFETY: 上面已确认 CPU 支持 AVX2，且 head 是 8 的整数倍
                unsafe { x86::i32(&samples[..head], out) };
                scalar::i32(&samples[head..], out);
                return;
            }
        }
        if x86::sse2_available() {
            let head = x86::head_len(samples.len(), x86::sse2_lanes());
            if head > 0 {
                // SAFETY: 上面已确认 CPU 支持 SSE2，且 head 是 4 的整数倍
                unsafe { x86::i32_sse2(&samples[..head], out) };
                scalar::i32(&samples[head..], out);
                return;
            }
        }
    }
    scalar::i32(samples, out);
}

fn int16(samples: &[f32], out: &mut Vec<u8>) {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if x86::avx2_available() {
            let head = x86::head_len(samples.len(), x86::avx2_lanes());
            if head > 0 {
                // SAFETY: 同上，head 是 8 的整数倍
                unsafe { x86::i16(&samples[..head], out) };
                scalar::i16(&samples[head..], out);
                return;
            }
        }
        if x86::sse2_available() {
            let head = x86::head_len(samples.len(), x86::sse2_lanes());
            if head > 0 {
                // SAFETY: 同上，head 是 4 的整数倍
                unsafe { x86::i16_sse2(&samples[..head], out) };
                scalar::i16(&samples[head..], out);
                return;
            }
        }
    }
    scalar::i16(samples, out);
}

/// 标量实现：所有平台的兜底，也是 SIMD 内核的测试基准。
/// 满刻度先乘 2^(n-1) 再饱和转整，与 cpal 的 f32 → 整数取法一致，两种模式不会打架。
mod scalar {
    use super::PcmFormat;

    /// 追加写入，调用方负责先清空。
    pub(super) fn f32(samples: &[f32], out: &mut Vec<u8>) {
        for &sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
    }

    pub(super) fn i32(samples: &[f32], out: &mut Vec<u8>) {
        let scale = PcmFormat::I32.full_scale();
        for &sample in samples {
            let scaled = (sample.clamp(-1.0, 1.0) * scale).round();
            out.extend_from_slice(&(scaled as i32).to_le_bytes());
        }
    }

    pub(super) fn i24(samples: &[f32], out: &mut Vec<u8>) {
        let scale = PcmFormat::I24.full_scale();
        for &sample in samples {
            let scaled = (sample.clamp(-1.0, 1.0) * scale).round() as i32;
            // 24 位有符号的上界是 2^23 − 1，而 +1.0 正好算到 2^23
            let value = scaled.clamp(-8_388_608, 8_388_607);
            out.extend_from_slice(&value.to_le_bytes()[..3]);
        }
    }

    pub(super) fn i16(samples: &[f32], out: &mut Vec<u8>) {
        let scale = PcmFormat::I16.full_scale();
        for &sample in samples {
            let scaled = (sample.clamp(-1.0, 1.0) * scale).round();
            out.extend_from_slice(&(scaled as i16).to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probes() -> Vec<Vec<f32>> {
        let fixed = vec![
            0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            0.25,
            -0.25,
            0.9999,
            -0.9999,
            1.5,
            -1.5,
            f32::NAN,
            f32::INFINITY,
            -0.0,
            1.0 / 3.0,
            -1.0 / 3.0,
            0.99999994,
        ];
        let ramp: Vec<f32> = (0..1024).map(|i| (i as f32 / 512.0) - 1.0).collect();
        vec![fixed, ramp]
    }

    fn value_at(bytes: &[u8], index: usize, format: PcmFormat) -> i64 {
        match format {
            PcmFormat::I16 => {
                let start = index * 2;
                i64::from(i16::from_le_bytes([bytes[start], bytes[start + 1]]))
            }
            PcmFormat::I24 => {
                let start = index * 3;
                let raw = i32::from_le_bytes([bytes[start], bytes[start + 1], bytes[start + 2], 0]);
                // 3 字节小端有符号：最高位是符号位
                i64::from(raw << 8 >> 8)
            }
            PcmFormat::I32 => {
                let start = index * 4;
                i64::from(i32::from_le_bytes(
                    bytes[start..start + 4].try_into().unwrap_or([0; 4]),
                ))
            }
            PcmFormat::F32 => {
                let start = index * 4;
                i64::from(
                    f32::from_le_bytes(bytes[start..start + 4].try_into().unwrap_or([0; 4]))
                        .to_bits(),
                )
            }
        }
    }

    #[test]
    fn every_format_writes_exactly_the_expected_number_of_bytes() {
        for samples in probes() {
            for format in [
                PcmFormat::F32,
                PcmFormat::I32,
                PcmFormat::I24,
                PcmFormat::I16,
            ] {
                let mut out = Vec::new();
                write(&samples, format, &mut out);
                assert_eq!(
                    out.len(),
                    samples.len() * format.bytes_per_sample(),
                    "{format:?} 的字节数不对"
                );
            }
        }
    }

    #[test]
    fn write_clears_whatever_was_there_before() {
        let mut out = vec![0xAA; 64];
        write(&[0.0, 0.0], PcmFormat::I16, &mut out);
        assert_eq!(out.len(), 4, "上一次的残留必须先清掉");
    }

    #[test]
    fn silence_is_all_zeros_and_full_scale_is_the_integer_ceiling() {
        let mut out = Vec::new();

        write(&[0.0], PcmFormat::I16, &mut out);
        assert_eq!(out, vec![0, 0]);
        write(&[1.0], PcmFormat::I16, &mut out);
        assert_eq!(out, vec![0xFF, 0x7F], "1.0 应当饱和到 i16::MAX");
        write(&[-1.0], PcmFormat::I16, &mut out);
        assert_eq!(out, vec![0x00, 0x80], "-1.0 应当到 i16::MIN");

        write(&[1.0], PcmFormat::I32, &mut out);
        assert_eq!(out, vec![0xFF, 0xFF, 0xFF, 0x7F]);
        write(&[-1.0], PcmFormat::I32, &mut out);
        assert_eq!(out, vec![0x00, 0x00, 0x00, 0x80]);

        write(&[1.0], PcmFormat::I24, &mut out);
        assert_eq!(out, vec![0xFF, 0xFF, 0x7F], "24 位的上界是 2^23 − 1");
        write(&[-1.0], PcmFormat::I24, &mut out);
        assert_eq!(out, vec![0x00, 0x00, 0x80]);
    }

    #[test]
    fn out_of_range_and_special_values_never_wrap_around() {
        let mut out = Vec::new();
        for format in [PcmFormat::I32, PcmFormat::I24, PcmFormat::I16] {
            let samples = [2.0_f32, -2.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
            write(&samples, format, &mut out);
            let decoded: Vec<i64> = (0..samples.len())
                .map(|index| value_at(&out, index, format))
                .collect();
            assert_eq!(decoded[0], decoded[0].max(0), "2.0 不该变成负数");
            assert_eq!(decoded[1], decoded[1].min(0), "-2.0 不该变成正数");
            assert_eq!(decoded[2], 0, "NaN 应当落到 0");
            assert!(decoded[3] > 0, "正无穷应当落到正的上界");
            assert!(decoded[4] < 0, "负无穷应当落到负的下界");
        }
    }

    #[test]
    fn f32_keeps_the_samples_untouched() {
        let samples = [0.5_f32, -0.25, 1.7, f32::MIN_POSITIVE];
        let mut out = Vec::new();
        write(&samples, PcmFormat::F32, &mut out);
        for (index, sample) in samples.iter().enumerate() {
            let start = index * 4;
            let decoded = f32::from_le_bytes(out[start..start + 4].try_into().unwrap());
            assert_eq!(decoded.to_bits(), sample.to_bits(), "F32 应当是位精确的");
        }
    }

    #[test]
    fn the_zero_copy_view_matches_the_sample_by_sample_path() {
        for samples in probes() {
            let mut written = Vec::new();
            write(&samples, PcmFormat::F32, &mut written);
            assert_eq!(f32_as_bytes(&samples), written.as_slice());
        }
        assert_eq!(f32_as_bytes(&[]), []);
    }

    /// 容差 1 LSB：SIMD 就近取偶、标量就近取远，差只出现在半整数上。
    #[test]
    fn simd_kernels_agree_with_the_scalar_reference() {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            if !x86::avx2_available() && !x86::sse2_available() {
                return; // 没有 SIMD 可测，标量就是唯一实现
            }
            #[cfg(target_arch = "x86_64")]
            assert!(
                x86::sse2_available(),
                "x86_64 必然带 SSE2，这里不该退化成空测"
            );

            for samples in probes() {
                for format in [PcmFormat::I32, PcmFormat::I16] {
                    let mut fast = Vec::new();
                    write(&samples, format, &mut fast);
                    let mut baseline = Vec::new();
                    match format {
                        PcmFormat::I32 => scalar::i32(&samples, &mut baseline),
                        PcmFormat::I16 => scalar::i16(&samples, &mut baseline),
                        _ => unreachable!(),
                    }

                    assert_eq!(fast.len(), baseline.len(), "{format:?} 长度不一致");
                    for index in 0..samples.len() {
                        let a = value_at(&fast, index, format);
                        let b = value_at(&baseline, index, format);
                        assert!(
                            (a - b).abs() <= 1,
                            "{format:?} 第 {index} 个样本差了 {}（SIMD {a} / 标量 {b}）",
                            (a - b).abs()
                        );
                    }
                }
            }
        }
    }

    /// 内核只处理整块，尾巴必须由标量补齐且不能错位。
    #[test]
    fn a_length_that_is_not_a_multiple_of_the_vector_width_stays_aligned() {
        for len in [1_usize, 3, 7, 8, 9, 15, 16, 17, 63, 100] {
            let samples: Vec<f32> = (0..len).map(|i| (i as f32 / len as f32) - 0.5).collect();
            let mut mixed = Vec::new();
            write(&samples, PcmFormat::I16, &mut mixed);

            let mut baseline = Vec::new();
            scalar::i16(&samples, &mut baseline);

            assert_eq!(mixed.len(), baseline.len(), "长度 {len} 时字节数不对");
            for index in 0..len {
                let a = value_at(&mixed, index, PcmFormat::I16);
                let b = value_at(&baseline, index, PcmFormat::I16);
                assert!(
                    (a - b).abs() <= 1,
                    "长度 {len} 时第 {index} 个样本错位或偏离"
                );
            }
        }
    }
}
