//! 声道布局与下混。系数取 ITU-R BS.775-1。
//! LFE 默认不并入立体声；例外是 2.1：那一路没有别的去处，折入并按录音时被抬高的 10 dB 折算回来。

/// 声道在布局里的角色。`Unknown` 只出现在已知布局之外的尾部。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    Fl,
    Fr,
    Fc,
    Lfe,
    Bl,
    Br,
    Sl,
    Sr,
    Unknown,
}

const MONO: [Role; 1] = [Role::Fc];
const STEREO: [Role; 2] = [Role::Fl, Role::Fr];
const SURROUND_21: [Role; 3] = [Role::Fl, Role::Fr, Role::Lfe];
const SURROUND_51: [Role; 6] = [Role::Fl, Role::Fr, Role::Fc, Role::Lfe, Role::Sl, Role::Sr];
const SURROUND_71: [Role; 8] = [
    Role::Fl,
    Role::Fr,
    Role::Fc,
    Role::Lfe,
    Role::Bl,
    Role::Br,
    Role::Sl,
    Role::Sr,
];

/// 交错声道数对应的布局。3 声道一律按 2.1 理解（FL FR LFE），不假设中置。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ChannelLayout {
    Mono,
    Stereo,
    Surround21,
    Surround51,
    Surround71,
    Other(usize),
}

impl ChannelLayout {
    pub(crate) const fn from_count(channels: usize) -> Self {
        match channels {
            0 | 1 => Self::Mono,
            2 => Self::Stereo,
            3 => Self::Surround21,
            6 => Self::Surround51,
            8 => Self::Surround71,
            other => Self::Other(other),
        }
    }

    /// 是否有一条独立的低频通道 —— LFE 低通只对它成立。
    pub(crate) const fn has_lfe(self) -> bool {
        match self {
            Self::Surround21 | Self::Surround51 | Self::Surround71 => true,
            // 未知布局只有前四路能按 2.1 之后的常见次序读出来
            Self::Other(channels) => channels > 3,
            Self::Mono | Self::Stereo => false,
        }
    }

    fn role(self, index: usize) -> Role {
        let table: &[Role] = match self {
            Self::Mono => &MONO,
            Self::Stereo => &STEREO,
            Self::Surround21 => &SURROUND_21,
            Self::Surround51 => &SURROUND_51,
            Self::Surround71 => &SURROUND_71,
            Self::Other(_) => &[Role::Fl, Role::Fr, Role::Fc, Role::Lfe],
        };
        match table.get(index) {
            Some(role) => *role,
            None => Role::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LfeConfig {
    /// 5.1 / 7.1 下混成立体声时是否也折入 LFE。默认关，沿用 ITU-R BS.775-1。
    pub include_lfe: bool,
    /// LFE 相对主声道的增益。0.316 ≈ −10 dB，正好抵消 LFE 录音时的抬高。
    pub gain: f32,
    /// LFE 低通截止频率。LFE 本身就到 120 Hz，这里只是兜底。
    pub cutoff_hz: f32,
}

impl Default for LfeConfig {
    fn default() -> Self {
        Self {
            include_lfe: false,
            gain: 0.316,
            cutoff_hz: 120.0,
        }
    }
}

/// 一阶低通。状态必须跨帧保留 —— 放进解码器会让每个解码包边界都重启滤波器，听得出来。
pub(crate) struct LfeLowPass {
    alpha: f32,
    state: f32,
}

impl LfeLowPass {
    pub(crate) fn new(cutoff_hz: f32, sample_rate: u32) -> Self {
        // alpha = dt / (rc + dt)。采样率或截止频率不可用时退化成直通。
        let alpha = if sample_rate == 0 || cutoff_hz <= 0.0 {
            1.0
        } else {
            let dt = 1.0 / sample_rate as f32;
            let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
            (dt / (rc + dt)).clamp(0.0, 1.0)
        };
        Self { alpha, state: 0.0 }
    }

    #[inline]
    fn process(&mut self, sample: f32) -> f32 {
        self.state += self.alpha * (sample - self.state);
        self.state
    }
}

/// 把一帧交错采样从源布局转到目标声道数。目标声道数就是输出设备的声道数。
pub(crate) fn convert_frame(
    src: &[f32],
    dst: &mut [f32],
    lfe: Option<&mut LfeLowPass>,
    config: LfeConfig,
) {
    dst.fill(0.0);
    if src.is_empty() || dst.is_empty() {
        return;
    }

    if src.len() == 1 {
        dst[0] = src[0];
        if dst.len() > 1 {
            dst[1] = src[0];
        }
        return;
    }

    if dst.len() <= 2 {
        let (left, right) = downmix_to_stereo(src, lfe, config);
        dst[0] = if dst.len() == 1 {
            f32::midpoint(left, right)
        } else {
            left
        };
        if dst.len() > 1 {
            dst[1] = right;
        }
        return;
    }

    // 目标也是多声道：按角色对位，源里缺的角色留静音。
    // 不能按序数照抄 —— 序数 2 在 2.1 是 LFE、在 5.1 是 FC，照抄会把低频塞进中置。
    let source = ChannelLayout::from_count(src.len());
    let target = ChannelLayout::from_count(dst.len());
    for (index, sample) in dst.iter_mut().enumerate() {
        let role = target.role(index);
        if role == Role::Unknown {
            continue;
        }
        if let Some(position) = (0..src.len()).find(|at| source.role(*at) == role) {
            *sample = src[position];
        }
    }
}

/// 多声道 → 立体声。
fn downmix_to_stereo(src: &[f32], lfe: Option<&mut LfeLowPass>, config: LfeConfig) -> (f32, f32) {
    match ChannelLayout::from_count(src.len()) {
        ChannelLayout::Stereo => (src[0], src[1]),
        // 2.1 的 LFE 没有别的去处，必折
        ChannelLayout::Surround21 => {
            let sub = fold_lfe(src[2], lfe, config);
            (src[0] + sub, src[1] + sub)
        }
        layout => surround_to_stereo(src, layout, lfe, config),
    }
}

/// 5.1 / 7.1 以及未知多声道布局的下混。LFE 是否参与由 [`LfeConfig::include_lfe`] 决定。
fn surround_to_stereo(
    src: &[f32],
    layout: ChannelLayout,
    lfe: Option<&mut LfeLowPass>,
    config: LfeConfig,
) -> (f32, f32) {
    const CENTER: f32 = 0.707;
    const SURROUND: f32 = 0.707;
    const BACK: f32 = 0.5;

    let at = |role: Role| -> f32 {
        (0..src.len())
            .find(|index| layout.role(*index) == role)
            .map_or(0.0, |index| src[index])
    };

    let (mut left, mut right) = (at(Role::Fl), at(Role::Fr));
    left += at(Role::Fc) * CENTER;
    right += at(Role::Fc) * CENTER;

    match layout {
        ChannelLayout::Surround51 => {
            left += at(Role::Sl) * SURROUND;
            right += at(Role::Sr) * SURROUND;
        }
        ChannelLayout::Surround71 => {
            left += at(Role::Sl) * SURROUND + at(Role::Bl) * BACK;
            right += at(Role::Sr) * SURROUND + at(Role::Br) * BACK;
        }
        _ => {}
    }

    // 声道越多叠加后越容易过载，按布局给一个归一化系数
    let normalize = match layout {
        ChannelLayout::Surround51 => 0.707,
        ChannelLayout::Surround71 => 0.667,
        _ => 0.8,
    };

    if config.include_lfe && layout.has_lfe() {
        let sub = fold_lfe(at(Role::Lfe), lfe, config);
        left += sub;
        right += sub;
    }

    // 不在这里 clamp：末级的软削波会处理过载，提前硬切会把那一步的优势抵消掉
    (left * normalize, right * normalize)
}

/// LFE 折进全频段声道前的处理：先低通，再按 −10 dB 折算。
fn fold_lfe(sample: f32, lfe: Option<&mut LfeLowPass>, config: LfeConfig) -> f32 {
    let filtered = match lfe {
        Some(filter) => filter.process(sample),
        None => sample,
    };
    filtered * config.gain
}

/// 软削波。|x| ≤ 0.95 位精确直通（绝大多数采样走这条），超出的按 tanh 压进 ±1。
/// 放在链路末级：下混、音量、淡出叠加之后才是真正的输出电平。
#[inline]
pub(crate) fn soft_clip(x: f32) -> f32 {
    const THRESHOLD: f32 = 0.95;
    let magnitude = x.abs();
    if magnitude <= THRESHOLD {
        return x;
    }
    let over = (magnitude - THRESHOLD) / (1.0 - THRESHOLD);
    (THRESHOLD + (1.0 - THRESHOLD) * (over * 0.5).tanh()).copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn convert(src: &[f32], dst_channels: usize) -> Vec<f32> {
        let mut dst = vec![0.0; dst_channels];
        convert_frame(src, &mut dst, None, LfeConfig::default());
        dst
    }

    #[test]
    fn stereo_to_stereo_passes_through_untouched() {
        assert_eq!(convert(&[0.1, -0.2], 2), vec![0.1, -0.2]);
    }

    #[test]
    fn mono_is_duplicated_to_both_channels() {
        assert_eq!(convert(&[0.25], 2), vec![0.25, 0.25]);
        // 单声道设备才一一对应
        assert_eq!(convert(&[0.25], 1), vec![0.25]);
    }

    #[test]
    fn stereo_downmixes_to_mono_by_averaging() {
        assert_eq!(convert(&[0.5, 0.25], 1), vec![0.375]);
    }

    /// 声道数取自容器头，坏文件可以写 0 —— 它不能变成除数。
    #[test]
    fn an_empty_frame_does_not_panic() {
        assert!(convert(&[], 2).iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn downmix_5_1_uses_itu_coefficients_and_drops_lfe() {
        // FL FR FC LFE SL SR —— LFE 那一路(0.9)默认不应出现在结果里
        let frame = [0.5_f32, -0.4, 0.2, 0.9, 0.1, 0.2];
        let out = convert(&frame, 2);

        let expected_l = (0.5 + 0.2 * 0.707 + 0.1 * 0.707) * 0.707;
        let expected_r = (-0.4 + 0.2 * 0.707 + 0.2 * 0.707) * 0.707;
        assert!(approx_eq(out[0], expected_l), "5.1 L = {}", out[0]);
        assert!(approx_eq(out[1], expected_r), "5.1 R = {}", out[1]);
    }

    #[test]
    fn downmix_7_1_uses_itu_coefficients() {
        // FL FR FC LFE BL BR SL SR
        let frame = [0.5_f32, -0.4, 0.2, 0.9, 0.1, -0.1, 0.3, 0.2];
        let out = convert(&frame, 2);

        let expected_l = (0.5 + 0.2 * 0.707 + 0.3 * 0.707 + 0.1 * 0.5) * 0.667;
        let expected_r = (-0.4 + 0.2 * 0.707 + 0.2 * 0.707 + (-0.1) * 0.5) * 0.667;
        assert!(approx_eq(out[0], expected_l), "7.1 L = {}", out[0]);
        assert!(approx_eq(out[1], expected_r), "7.1 R = {}", out[1]);
    }

    #[test]
    fn enabling_lfe_adds_it_on_top_of_the_itu_downmix() {
        let frame = [0.5_f32, -0.4, 0.2, 0.9, 0.1, 0.2];
        let config = LfeConfig {
            include_lfe: true,
            // 直通用例：把增益设成 1，低通关掉，只看折不折进来
            gain: 1.0,
            cutoff_hz: 0.0,
        };
        let mut dst = vec![0.0; 2];
        convert_frame(&frame, &mut dst, None, config);

        let sub = 0.9;
        let expected_l = (0.5 + 0.2 * 0.707 + 0.1 * 0.707 + sub) * 0.707;
        assert!(approx_eq(dst[0], expected_l), "L = {}", dst[0]);
    }

    #[test]
    fn surround_21_folds_lfe_into_stereo_at_minus_ten_db() {
        // FL FR LFE
        let frame = [0.4_f32, -0.3, 0.8];
        let mut dst = vec![0.0; 2];
        convert_frame(&frame, &mut dst, None, LfeConfig::default());

        let sub = 0.8 * 0.316;
        assert!(approx_eq(dst[0], 0.4 + sub), "L = {}", dst[0]);
        assert!(approx_eq(dst[1], -0.3 + sub), "R = {}", dst[1]);
    }

    #[test]
    fn surround_21_to_three_channels_keeps_lfe_in_place() {
        let frame = [0.4_f32, -0.3, 0.8];
        assert_eq!(convert(&frame, 3), vec![0.4, -0.3, 0.8]);
    }

    #[test]
    fn surround_21_lands_lfe_in_the_lfe_slot_not_the_centre() {
        // 2.1 → 5.1：序数 2 是 LFE，绝不能照抄进 FC
        let frame = [0.4_f32, -0.3, 0.8];
        assert_eq!(convert(&frame, 6), vec![0.4, -0.3, 0.0, 0.8, 0.0, 0.0]);
    }

    #[test]
    fn surround_51_to_six_channels_is_a_passthrough() {
        let frame = [0.5_f32, -0.4, 0.2, 0.9, 0.1, 0.2];
        assert_eq!(convert(&frame, 6), frame.to_vec());
    }

    #[test]
    fn stereo_to_surround_only_fills_the_front_pair() {
        // 声源里没有低频，不臆造一路出来
        assert_eq!(
            convert(&[0.5, -0.4], 6),
            vec![0.5, -0.4, 0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn seven_one_to_five_one_keeps_the_side_pair_and_drops_the_back_pair() {
        // FL FR FC LFE BL BR SL SR → FL FR FC LFE SL SR：侧环绕有位可去，后环绕没有
        let frame = [0.5_f32, -0.4, 0.2, 0.9, 0.1, -0.1, 0.3, 0.2];
        assert_eq!(convert(&frame, 6), vec![0.5, -0.4, 0.2, 0.9, 0.3, 0.2]);
    }

    #[test]
    fn soft_clip_is_bit_exact_below_the_threshold() {
        for x in [0.0_f32, 0.1, 0.5, -0.9, 0.95, -0.95] {
            assert_eq!(soft_clip(x).to_bits(), x.to_bits(), "{x} 不应被改动");
        }
    }

    #[test]
    fn soft_clip_compresses_towards_the_ceiling() {
        let mut previous = soft_clip(0.95);
        for x in [0.96_f32, 1.0, 1.05, 1.2, 1.5] {
            let y = soft_clip(x);
            assert!(y > previous, "应当严格递增：{previous} -> {y}");
            assert!(y <= 1.0, "不该越过满刻度：{y}");
            assert!(y.abs() < x.abs(), "幅度应当被压小：{x} -> {y}");
            previous = y;
        }

        // 再往上 f32 的 tanh 已经饱和，输出停在 ±1，绝不越过
        for x in [1.5_f32, 2.0, 10.0, 100.0] {
            let y = soft_clip(x);
            assert!(y >= previous, "饱和段也不该回落：{previous} -> {y}");
            assert!(y <= 1.0, "不该越过满刻度：{y}");
        }
        assert_eq!(soft_clip(100.0), 1.0);
        assert_eq!(soft_clip(-100.0), -1.0);

        for x in [0.96_f32, 1.5, 100.0] {
            assert_eq!(soft_clip(-x).to_bits(), (-soft_clip(x)).to_bits());
        }
    }

    #[test]
    fn lfe_low_pass_keeps_state_across_frames() {
        let config = LfeConfig {
            include_lfe: true,
            gain: 1.0,
            cutoff_hz: 120.0,
        };
        let frame = [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0];
        let mut filter = LfeLowPass::new(config.cutoff_hz, 48_000);
        let mut first = vec![0.0; 2];
        let mut second = vec![0.0; 2];
        convert_frame(&frame, &mut first, Some(&mut filter), config);
        convert_frame(&frame, &mut second, Some(&mut filter), config);

        let alpha = 1.0 / (1.0 + 48_000.0 / (2.0 * std::f32::consts::PI * 120.0));
        assert!(
            approx_eq(first[0], alpha * 0.707),
            "第一帧应当只充到一阶低通的一小步，实际 {}",
            first[0]
        );
        assert!(
            approx_eq(second[0], (2.0 * alpha - alpha * alpha) * 0.707),
            "第二帧应当接着上一帧的状态往上走，实际 {}",
            second[0]
        );
    }
}
