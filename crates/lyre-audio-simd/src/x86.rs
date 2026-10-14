//! `x86` / `x86_64` 的 PCM 内核。整个 crate 只有这里出现 `unsafe`。
//!
//! 两处与标量基准的差别，都是刻意的：
//! - 四舍五入：`cvtps` 就近取偶，标量 `.round()` 就近取远，最多差一个最低位；
//! - NaN：标量的 `as` 转换给出 0，而 `min`/`max` 遇到 NaN 会返回另一侧操作数（变成一个
//!   满刻度值），所以这里必须显式把 NaN 通道置零，否则一次解码异常就是一记满幅爆音。
//!
//! 满刻度的饱和值两条路径完全一致。

#[cfg(target_arch = "x86")]
use std::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use std::sync::LazyLock;

/// 向量宽度（样本数）：AVX2 八路，SSE2 四路。
const AVX2_LANES: usize = 8;
const SSE2_LANES: usize = 4;

/// 能整块交给 SIMD 的前缀长度，剩下的尾巴由标量补齐。
pub(super) const fn head_len(len: usize, lanes: usize) -> usize {
    len / lanes * lanes
}

pub(super) const fn avx2_lanes() -> usize {
    AVX2_LANES
}

pub(super) const fn sse2_lanes() -> usize {
    SSE2_LANES
}

pub(super) fn avx2_available() -> bool {
    static AVAILABLE: LazyLock<bool> = LazyLock::new(|| is_x86_feature_detected!("avx2"));
    *AVAILABLE
}

pub(super) fn sse2_available() -> bool {
    static AVAILABLE: LazyLock<bool> = LazyLock::new(|| is_x86_feature_detected!("sse2"));
    *AVAILABLE
}

/// 32 位整数，AVX2 八路。
///
/// 约定：`samples.len()` 是 [`AVX2_LANES`] 的整数倍，字节追加写到 `out` 末尾。
///
/// # Safety
/// 调用方必须先用 [`avx2_available`] 确认 CPU 支持 AVX2。
#[target_feature(enable = "avx2")]
pub(super) unsafe fn i32(samples: &[f32], out: &mut Vec<u8>) {
    debug_assert_eq!(samples.len() % AVX2_LANES, 0);

    let base = out.len();
    out.resize(base + samples.len() * 4, 0);

    let negative = _mm256_set1_ps(-1.0);
    let positive = _mm256_set1_ps(1.0);
    let scale = _mm256_set1_ps(2_147_483_648.0);
    let ceiling = _mm256_set1_epi32(i32::MAX);

    // SAFETY: 调用方已确认 CPU 支持 AVX2；`base` 之后是刚 resize 出来的
    // samples.len() * 4 字节，按 8 个采样一轮正好写满，不会越界。
    unsafe {
        let dst = out.as_mut_ptr().add(base);

        let mut index = 0;
        while index < samples.len() {
            let raw = _mm256_loadu_ps(samples.as_ptr().add(index));
            // min/max 遇 NaN 会折成满刻度，先挑出来置零（见模块文档）
            let raw = _mm256_andnot_ps(_mm256_cmp_ps::<{ _CMP_UNORD_Q }>(raw, raw), raw);
            let clamped = _mm256_min_ps(_mm256_max_ps(raw, negative), positive);
            let scaled = _mm256_mul_ps(clamped, scale);
            // +1.0 正好算到 2^31，越界时 cvtps 给的是 INT_MIN，必须补成 INT_MAX
            let overflow = _mm256_cmp_ps::<{ _CMP_GE_OQ }>(scaled, scale);
            let converted = _mm256_cvtps_epi32(scaled);
            let fixed = _mm256_blendv_epi8(converted, ceiling, _mm256_castps_si256(overflow));

            _mm256_storeu_si256(dst.add(index * 4).cast::<__m256i>(), fixed);
            index += AVX2_LANES;
        }
    }
}

/// 16 位整数，AVX2 八路。
///
/// # Safety
/// 调用方必须先用 [`avx2_available`] 确认 CPU 支持 AVX2。
#[target_feature(enable = "avx2")]
pub(super) unsafe fn i16(samples: &[f32], out: &mut Vec<u8>) {
    debug_assert_eq!(samples.len() % AVX2_LANES, 0);

    let base = out.len();
    out.resize(base + samples.len() * 2, 0);

    let negative = _mm256_set1_ps(-1.0);
    let positive = _mm256_set1_ps(1.0);
    let scale = _mm256_set1_ps(32_768.0);

    // SAFETY: 同 `i32`，每轮写 8 个采样 = 16 字节。
    unsafe {
        let dst = out.as_mut_ptr().add(base);

        let mut index = 0;
        while index < samples.len() {
            let raw = _mm256_loadu_ps(samples.as_ptr().add(index));
            // min/max 遇 NaN 会折成满刻度，先挑出来置零（见模块文档）
            let raw = _mm256_andnot_ps(_mm256_cmp_ps::<{ _CMP_UNORD_Q }>(raw, raw), raw);
            let clamped = _mm256_min_ps(_mm256_max_ps(raw, negative), positive);
            let converted = _mm256_cvtps_epi32(_mm256_mul_ps(clamped, scale));

            // `packs` 自带饱和，+1.0 算出来的 32768 会落回 32767，与标量的饱和转换一致。
            // 结果按 128 位通道交错，把两个通道的低 64 位前后拼起来才是有序的 8 个 i16。
            let packed = _mm256_packs_epi32(converted, converted);
            let ordered = _mm256_permute4x64_epi64::<0b0000_1000>(packed);
            _mm_storeu_si128(
                dst.add(index * 2).cast::<__m128i>(),
                _mm256_castsi256_si128(ordered),
            );
            index += AVX2_LANES;
        }
    }
}

/// 32 位整数，SSE2 四路。没有 AVX2 时的退路。
///
/// # Safety
/// 调用方必须先用 [`sse2_available`] 确认 CPU 支持 SSE2。
#[target_feature(enable = "sse2")]
pub(super) unsafe fn i32_sse2(samples: &[f32], out: &mut Vec<u8>) {
    debug_assert_eq!(samples.len() % SSE2_LANES, 0);

    let base = out.len();
    out.resize(base + samples.len() * 4, 0);

    let negative = _mm_set1_ps(-1.0);
    let positive = _mm_set1_ps(1.0);
    let scale = _mm_set1_ps(2_147_483_648.0);
    let ceiling = _mm_set1_epi32(i32::MAX);

    // SAFETY: 调用方已确认 CPU 支持 SSE2；写入区间同 `i32`，每轮 4 个采样 = 16 字节。
    unsafe {
        let dst = out.as_mut_ptr().add(base);

        let mut index = 0;
        while index < samples.len() {
            let raw = _mm_loadu_ps(samples.as_ptr().add(index));
            // min/max 遇 NaN 会折成满刻度，先挑出来置零（见模块文档）
            let raw = _mm_andnot_ps(_mm_cmpunord_ps(raw, raw), raw);
            let clamped = _mm_min_ps(_mm_max_ps(raw, negative), positive);
            let scaled = _mm_mul_ps(clamped, scale);
            let converted = _mm_cvtps_epi32(scaled);
            // SSE2 没有 blendv，用掩码位运算做选择
            let overflow = _mm_castps_si128(_mm_cmpge_ps(scaled, scale));
            let fixed = _mm_or_si128(
                _mm_and_si128(overflow, ceiling),
                _mm_andnot_si128(overflow, converted),
            );

            _mm_storeu_si128(dst.add(index * 4).cast::<__m128i>(), fixed);
            index += SSE2_LANES;
        }
    }
}

/// 16 位整数，SSE2 四路。
///
/// # Safety
/// 调用方必须先用 [`sse2_available`] 确认 CPU 支持 SSE2。
#[target_feature(enable = "sse2")]
pub(super) unsafe fn i16_sse2(samples: &[f32], out: &mut Vec<u8>) {
    debug_assert_eq!(samples.len() % SSE2_LANES, 0);

    let base = out.len();
    out.resize(base + samples.len() * 2, 0);

    let negative = _mm_set1_ps(-1.0);
    let positive = _mm_set1_ps(1.0);
    let scale = _mm_set1_ps(32_768.0);

    // SAFETY: 调用方已确认 CPU 支持 SSE2；每轮写 4 个采样 = 8 字节。
    unsafe {
        let dst = out.as_mut_ptr().add(base);

        let mut index = 0;
        while index < samples.len() {
            let raw = _mm_loadu_ps(samples.as_ptr().add(index));
            // min/max 遇 NaN 会折成满刻度，先挑出来置零（见模块文档）
            let raw = _mm_andnot_ps(_mm_cmpunord_ps(raw, raw), raw);
            let clamped = _mm_min_ps(_mm_max_ps(raw, negative), positive);
            let converted = _mm_cvtps_epi32(_mm_mul_ps(clamped, scale));

            // packs 自带饱和：8 个 i16 里前 4 个就是结果，低 64 位直接存
            let packed = _mm_packs_epi32(converted, converted);
            _mm_storel_epi64(dst.add(index * 2).cast::<__m128i>(), packed);
            index += SSE2_LANES;
        }
    }
}
