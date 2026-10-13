//! 多声道下混，系数取 ITU-R BS.775-1。LFE 不混入立体声
//! （LFE 应该有办法加回来，我可以加 limit 和衰减-15 dB这样，而且 decode 部分好像还没有考虑 2.1 声道的音响）。

/// 5.1 / 7.1 环绕声到立体声的下混。声道顺序：5.1 = FL FR FC LFE SL SR，7.1 = FL FR FC LFE BL BR SL SR。
pub(crate) fn downmix_surround_to_stereo(
    samples: &[f32],
    src_ch: usize,
    frame: usize,
) -> (f32, f32) {
    const CENTER_MIX: f32 = 0.707;
    const SURROUND_MIX: f32 = 0.707;
    const BACK_MIX: f32 = 0.5;

    let start = frame * src_ch;

    let fl = samples[start];
    let fr = samples[start + 1];
    let fc = if src_ch > 2 { samples[start + 2] } else { 0.0 };
    let _lfe = if src_ch > 3 { samples[start + 3] } else { 0.0 };

    let (mut left, mut right) = (fl, fr);

    left += fc * CENTER_MIX;
    right += fc * CENTER_MIX;

    match src_ch {
        6 => {
            let sl = samples[start + 4];
            let sr = samples[start + 5];
            left += sl * SURROUND_MIX;
            right += sr * SURROUND_MIX;
        }
        8 => {
            let bl = samples[start + 4];
            let br = samples[start + 5];
            let sl = samples[start + 6];
            let sr = samples[start + 7];
            left += sl * SURROUND_MIX + bl * BACK_MIX;
            right += sr * SURROUND_MIX + br * BACK_MIX;
        }
        _ => {}
    }

    // 声道越多，叠加后越容易过载，按布局给一个归一化系数
    let normalize = match src_ch {
        6 => 0.707,
        8 => 0.667,
        _ => 0.8,
    };

    (
        (left * normalize).clamp(-1.0, 1.0),
        (right * normalize).clamp(-1.0, 1.0),
    )
}

/// 把任意声道数的交错采样就地转成立体声。
/// 单声道复制成双声道，双声道原样返回，其余走 [`downmix_surround_to_stereo`]。
pub(crate) fn to_stereo_in_place(samples: &mut Vec<f32>, src_ch: usize) {
    if src_ch == 2 || samples.is_empty() {
        return;
    }

    if src_ch == 1 {
        let frames = samples.len();
        samples.resize(frames * 2, 0.0);
        // 写入位置恒不小于读取位置，倒序遍历才不会覆盖还没读到的样本
        for f in (0..frames).rev() {
            let s = samples[f];
            samples[2 * f] = s;
            samples[2 * f + 1] = s;
        }
        return;
    }

    if src_ch == 0 {
        // 声道数取自容器头，0 是坏数据；静音本身由解码器报出来。
        samples.clear();
        return;
    }

    let frames = samples.len() / src_ch;
    let mut write = 0usize;
    for f in 0..frames {
        let (left, right) = downmix_surround_to_stereo(samples, src_ch, f);
        samples[write] = left;
        samples[write + 1] = right;
        write += 2;
    }
    samples.truncate(write);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn stereo_passes_through_untouched() {
        let mut samples = vec![0.1, -0.2, 0.3, -0.4];
        to_stereo_in_place(&mut samples, 2);
        assert_eq!(samples, vec![0.1, -0.2, 0.3, -0.4]);
    }

    #[test]
    fn mono_is_duplicated_to_both_channels() {
        let mut samples = vec![0.25, -0.5, 0.75];
        to_stereo_in_place(&mut samples, 1);
        assert_eq!(samples, vec![0.25, 0.25, -0.5, -0.5, 0.75, 0.75]);
    }

    /// 声道数取自容器头，坏文件可以写 0 —— 它不能变成除数。
    #[test]
    fn zero_channel_count_does_not_divide_by_zero() {
        let mut samples = vec![0.25, -0.25];
        to_stereo_in_place(&mut samples, 0);
        assert!(samples.is_empty());
    }

    #[test]
    fn downmix_5_1_uses_itu_coefficients_and_drops_lfe() {
        // FL FR FC LFE SL SR —— LFE 那一路(0.9)不应出现在结果里
        let frame = [0.5_f32, -0.4, 0.2, 0.9, 0.1, 0.2];
        let (l, r) = downmix_surround_to_stereo(&frame, 6, 0);

        let expected_l = (0.5 + 0.2 * 0.707 + 0.1 * 0.707) * 0.707;
        let expected_r = (-0.4 + 0.2 * 0.707 + 0.2 * 0.707) * 0.707;
        assert!(approx_eq(l, expected_l), "5.1 L = {l}, 期望 {expected_l}");
        assert!(approx_eq(r, expected_r), "5.1 R = {r}, 期望 {expected_r}");
    }

    #[test]
    fn downmix_7_1_uses_itu_coefficients() {
        // FL FR FC LFE BL BR SL SR
        let frame = [0.5_f32, -0.4, 0.2, 0.9, 0.1, -0.1, 0.3, 0.2];
        let (l, r) = downmix_surround_to_stereo(&frame, 8, 0);

        let expected_l = (0.5 + 0.2 * 0.707 + 0.3 * 0.707 + 0.1 * 0.5) * 0.667;
        let expected_r = (-0.4 + 0.2 * 0.707 + 0.2 * 0.707 + (-0.1) * 0.5) * 0.667;
        assert!(approx_eq(l, expected_l), "7.1 L = {l}, 期望 {expected_l}");
        assert!(approx_eq(r, expected_r), "7.1 R = {r}, 期望 {expected_r}");
    }

    #[test]
    fn downmix_never_exceeds_full_scale() {
        // 全 1.0 的 7.1 帧是最坏情况，归一化之后必须仍在 [-1, 1] 内
        let frame = [1.0_f32; 8];
        let (l, r) = downmix_surround_to_stereo(&frame, 8, 0);
        assert!(l.abs() <= 1.0 && r.abs() <= 1.0, "l={l} r={r}");
    }

    #[test]
    fn to_stereo_in_place_handles_multiple_frames() {
        // 两帧 6 声道 → 两帧立体声，逐帧与 downmix 一致
        let src_ch = 6usize;
        let mut samples = Vec::new();
        for f in 0..2 {
            let base = f as f32 * 0.1;
            samples.extend_from_slice(&[0.5 + base, -0.4 - base, 0.2, 0.9, 0.1 + base, 0.2 - base]);
        }
        let original = samples.clone();

        to_stereo_in_place(&mut samples, src_ch);

        assert_eq!(samples.len(), 4);
        for f in 0..2 {
            let (l, r) = downmix_surround_to_stereo(&original, src_ch, f);
            assert!(approx_eq(samples[f * 2], l), "第 {f} 帧左声道不一致");
            assert!(approx_eq(samples[f * 2 + 1], r), "第 {f} 帧右声道不一致");
        }
    }
}
