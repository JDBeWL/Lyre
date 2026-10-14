//! 独占格式协商与流初始化。
//!
//! 协商顺序是这里最要紧的一点：**先要声源自己的采样率与声道数**。要到了就是位完美直出
//! （样本不重采样、不换声道，原样进硬件）；要不到才按设备混音格式、再按常用档位往下降。
//! 参考实现是反过来从设备默认口径开始试的 —— 那样 44.1k 的素材落在 48k 设备上必然被重采样。

use std::time::Duration;

use lyre_audio_simd::PcmFormat;
use wasapi::{
    AudioClient, AudioRenderClient, Device, DeviceEnumerator, Direction, Handle, SampleType,
    StreamMode, WaveFormat, initialize_mta, make_simple_channelmask,
};

use crate::error::{ExclusiveError, Result};

/// 依次尝试的位深。
const BIT_DEPTHS: [(usize, SampleType); 4] = [
    (32, SampleType::Float),
    (32, SampleType::Int),
    (24, SampleType::Int),
    (16, SampleType::Int),
];

/// 退让时依次尝试的采样率。
const FALLBACK_RATES: [u32; 11] = [
    384_000, 352_800, 192_000, 176_400, 96_000, 88_200, 48_000, 44_100, 32_000, 22_050, 16_000,
];

/// 初始化失败时的重试次数。设备刚被别的程序放开、驱动还在收尾，重试几次常常就成了。
const INIT_RETRIES: usize = 3;

/// 单次协商的总时限。候选格式有几十种，每种都重试三次会把用户晾在那儿好几秒 ——
/// 超时就直接认输，报出来的原因也比「等太久」有用。
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(3);

/// 独占模式下设备要求的周期，按 128 字节对齐 —— Intel HDA 一类设备不对齐会直接拒绝初始化。
const PERIOD_ALIGN_BYTES: u32 = 128;

/// 协商出来的独占格式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExclusiveFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: PcmFormat,
    /// 一个采样帧的字节数，等于声道数 × 每个采样的字节数。
    pub block_align: u16,
}

impl ExclusiveFormat {
    /// 口径与声源完全一致：中间不做重采样也不做声道转换，样本原样进硬件。
    pub const fn is_bit_perfect_for(self, sample_rate: u32, channels: u16) -> bool {
        self.sample_rate == sample_rate && self.channels == channels
    }

    /// 描述用，UI 上显示「192000 Hz / 2 ch / 32-bit float」这类文字。
    pub const fn describe(self) -> &'static str {
        match self.samples {
            PcmFormat::F32 => "32-bit float",
            PcmFormat::I32 => "32-bit int",
            PcmFormat::I24 => "24-bit int",
            PcmFormat::I16 => "16-bit int",
        }
    }
}

/// 初始化好的独占输出。整套 COM 对象都归渲染线程所有，只在那条线程上使用。
pub struct ExclusiveStream {
    pub client: AudioClient,
    pub render: AudioRenderClient,
    pub event: Handle,
    pub format: ExclusiveFormat,
    pub device_name: String,
    /// 这条流真正落在哪台设备上（WASAPI endpoint ID，不含 host 前缀）。
    /// 选定设备已拔出时 `find_device` 会退回默认设备，所以它与请求的设备可能不是同一台。
    pub device_endpoint: String,
}

impl ExclusiveStream {
    /// 打开独占流。`device_id` 是 `lyre_audio` 那套 `host:endpoint` 形式的稳定标识，
    /// `None` 表示系统默认设备；`preferred` 是声源的口径。
    pub fn open(device_id: Option<&str>, preferred: Option<(u32, u16)>) -> Result<Self> {
        // COM 必须先在这个线程上初始化成 MTA，否则后面每个调用都会失败
        if !initialize_mta().is_ok() {
            log::warn!("独占输出：COM 多线程单元初始化未成功，继续尝试");
        }

        let enumerator = DeviceEnumerator::new()
            .map_err(|err| ExclusiveError::Initialize(format!("创建设备枚举器失败：{err:?}")))?;
        let (device, device_name, device_endpoint) = find_device(&enumerator, device_id)?;

        let mut client = device
            .get_iaudioclient()
            .map_err(|err| ExclusiveError::Initialize(format!("取音频客户端失败：{err:?}")))?;

        let mix = client
            .get_mixformat()
            .map_err(|err| ExclusiveError::Initialize(format!("取混音格式失败：{err:?}")))?;
        let mix_rate = mix.get_samplespersec();
        let mix_channels = mix.get_nchannels();

        let (_default_period, min_period) = client
            .get_device_period()
            .map_err(|err| ExclusiveError::Initialize(format!("取设备周期失败：{err:?}")))?;

        let mut last_error = String::new();
        let started = std::time::Instant::now();
        for (rate, channels) in candidates(preferred, mix_rate, mix_channels) {
            if started.elapsed() >= NEGOTIATION_TIMEOUT {
                last_error = format!("尝试 {NEGOTIATION_TIMEOUT:?} 后仍未成功（{last_error}）");
                break;
            }

            for (bits, sample_type) in BIT_DEPTHS {
                let requested = wave_format(bits, sample_type, rate, channels);
                // `is_supported_exclusive_with_quirks` 会连通道掩码的变体一起试，
                // 返回设备真正接受的那一份（可能与我们请求的不完全一样）
                let Ok(accepted) = client.is_supported_exclusive_with_quirks(&requested) else {
                    continue;
                };
                let Some(format) = interpret(&accepted) else {
                    continue;
                };

                match initialize(&device, &accepted, min_period) {
                    Ok(client) => {
                        let event = client.set_get_eventhandle().map_err(|err| {
                            ExclusiveError::Initialize(format!("取事件句柄失败：{err:?}"))
                        })?;
                        let render = client.get_audiorenderclient().map_err(|err| {
                            ExclusiveError::Initialize(format!("取渲染客户端失败：{err:?}"))
                        })?;

                        log::info!(
                            "独占输出已建立：{device_name} @ {} Hz / {} ch / {}",
                            format.sample_rate,
                            format.channels,
                            format.describe()
                        );
                        return Ok(Self {
                            client,
                            render,
                            event,
                            format,
                            device_name,
                            device_endpoint,
                        });
                    }
                    Err(error) => {
                        last_error = error.to_string();
                        // 这台设备上的这次尝试失败了，重新取一个客户端再试下一个格式
                        client = device.get_iaudioclient().map_err(|err| {
                            ExclusiveError::Initialize(format!("重取音频客户端失败：{err:?}"))
                        })?;
                    }
                }
            }
        }

        Err(ExclusiveError::Unsupported(format!(
            "{device_name} 上找不到可用的独占格式（{last_error}）"
        )))
    }
}

/// 按 endpoint 标识找设备，找不到再退回友好名，都没有就用系统默认设备。
/// 一并返回实际那台设备的 endpoint —— 退回默认之后两者就不是同一个了。
fn find_device(
    enumerator: &DeviceEnumerator,
    device_id: Option<&str>,
) -> Result<(Device, String, String)> {
    let Some(id) = device_id else {
        let device = enumerator
            .get_default_device(&Direction::Render)
            .map_err(|err| ExclusiveError::Initialize(format!("取默认输出设备失败：{err:?}")))?;
        let name = device
            .get_friendlyname()
            .unwrap_or_else(|_| "默认输出设备".to_owned());
        let endpoint = device.get_id().unwrap_or_default();
        return Ok((device, name, endpoint));
    };

    let wanted = endpoint_of(id);
    let collection = enumerator
        .get_device_collection(&Direction::Render)
        .map_err(|err| ExclusiveError::Initialize(format!("枚举输出设备失败：{err:?}")))?;

    let mut by_name: Option<Device> = None;
    let wanted_name = lyre_audio::device::device_name(id);
    for device in collection.into_iter().flatten() {
        if device.get_id().is_ok_and(|found| found == wanted) {
            let name = device
                .get_friendlyname()
                .unwrap_or_else(|_| wanted.to_owned());
            return Ok((device, name, wanted.to_owned()));
        }
        if by_name.is_none()
            && let Some(wanted_name) = &wanted_name
            && device
                .get_friendlyname()
                .is_ok_and(|name| name == *wanted_name)
        {
            by_name = Some(device);
        }
    }

    if let Some(device) = by_name {
        let name = device
            .get_friendlyname()
            .unwrap_or_else(|_| "输出设备".to_owned());
        let endpoint = device.get_id().unwrap_or_default();
        return Ok((device, name, endpoint));
    }

    log::warn!("选定的输出设备已不可用，独占输出回退到系统默认设备");
    let device = enumerator
        .get_default_device(&Direction::Render)
        .map_err(|err| ExclusiveError::Initialize(format!("取默认输出设备失败：{err:?}")))?;
    let name = device
        .get_friendlyname()
        .unwrap_or_else(|_| "默认输出设备".to_owned());
    let endpoint = device.get_id().unwrap_or_default();
    Ok((device, name, endpoint))
}

/// `host:endpoint` 里的 endpoint 部分。cpal 的稳定标识带 host 前缀，WASAPI 自己不带。
pub(crate) fn endpoint_of(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, endpoint)| endpoint)
}

/// 候选口径，按优先级：声源自己的 → 设备混音格式 → 常用档位（各带上原声道数与 2 声道）。
fn candidates(preferred: Option<(u32, u16)>, mix_rate: u32, mix_channels: u16) -> Vec<(u32, u16)> {
    let mut out: Vec<(u32, u16)> = Vec::new();
    let mut push = |rate: u32, channels: u16| {
        if rate > 0 && channels > 0 && !out.contains(&(rate, channels)) {
            out.push((rate, channels));
        }
    };

    let wanted_channels = preferred.map_or(mix_channels.max(2), |(_, channels)| channels);
    if let Some((rate, channels)) = preferred {
        push(rate, channels);
    }
    push(mix_rate, mix_channels);
    for rate in FALLBACK_RATES {
        push(rate, wanted_channels);
        push(rate, 2);
    }
    out
}

fn wave_format(
    bits: usize,
    sample_type: SampleType,
    sample_rate: u32,
    channels: u16,
) -> WaveFormat {
    let channels = channels as usize;
    // 两声道以内用 WAVEFORMATEX 就够；多声道必须带通道掩码，否则设备多半不认
    let mask = (channels > 2).then(|| make_simple_channelmask(channels));
    WaveFormat::new(
        bits,
        bits,
        &sample_type,
        sample_rate as usize,
        channels,
        mask,
    )
}

/// 把设备接受的 `WaveFormat` 读回成我们的格式。认不出的容器直接跳过。
fn interpret(accepted: &WaveFormat) -> Option<ExclusiveFormat> {
    let bits = accepted.get_bitspersample();
    let valid = accepted.get_validbitspersample();
    // 24-in-32 这类「容器比有效位宽」的排布我们不写：写满 32 位会让音量差 256 倍
    if valid != 0 && valid != bits {
        return None;
    }

    let sample_type = accepted.get_subformat().ok()?;
    let samples = match (bits, sample_type) {
        (32, SampleType::Float) => PcmFormat::F32,
        (32, SampleType::Int) => PcmFormat::I32,
        (24, SampleType::Int) => PcmFormat::I24,
        (16, SampleType::Int) => PcmFormat::I16,
        _ => return None,
    };

    let channels = accepted.get_nchannels();
    let block_align = accepted.get_blockalign();
    if channels == 0 || block_align != u32::from(channels) * samples.bytes_per_sample() as u32 {
        return None;
    }

    Some(ExclusiveFormat {
        sample_rate: accepted.get_samplespersec(),
        channels,
        samples,
        block_align: block_align as u16,
    })
}

/// 初始化客户端，失败就重建客户端重试。
fn initialize(device: &Device, accepted: &WaveFormat, min_period: i64) -> Result<AudioClient> {
    // 官方示例针对 Intel HDA 一类设备要求周期按字节对齐，不对齐会直接拒绝初始化
    let period = device
        .get_iaudioclient()
        .ok()
        .and_then(|client| {
            client
                .calculate_aligned_period_near(
                    min_period.max(1),
                    Some(PERIOD_ALIGN_BYTES),
                    accepted,
                )
                .ok()
        })
        .unwrap_or(min_period.max(1));
    let mode = StreamMode::EventsExclusive { period_hns: period };

    let mut last_error = String::new();
    let mut client = device
        .get_iaudioclient()
        .map_err(|err| ExclusiveError::Initialize(format!("取音频客户端失败：{err:?}")))?;

    for attempt in 1..=INIT_RETRIES {
        match client.initialize_client(accepted, &Direction::Render, &mode) {
            Ok(()) => return Ok(client),
            Err(err) => {
                last_error = format!("{err:?}");
                log::warn!("独占初始化第 {attempt} 次失败：{last_error}");
                if attempt < INIT_RETRIES {
                    std::thread::sleep(std::time::Duration::from_millis(100 * attempt as u64));
                    client = device.get_iaudioclient().map_err(|err| {
                        ExclusiveError::Initialize(format!("重取音频客户端失败：{err:?}"))
                    })?;
                }
            }
        }
    }

    Err(ExclusiveError::Initialize(last_error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endpoint_is_stripped_from_the_stable_id() {
        assert_eq!(
            endpoint_of("wasapi:{0.0.0.00000000}.{abc}"),
            "{0.0.0.00000000}.{abc}"
        );
        // 没有 host 前缀时原样返回，其余平台的标识不该被截断
        assert_eq!(endpoint_of("Speakers"), "Speakers");
    }

    #[test]
    fn the_source_format_is_preferred_over_anything_the_device_likes() {
        let list = candidates(Some((44_100, 2)), 48_000, 2);
        assert_eq!(list[0], (44_100, 2), "声源口径必须排在最前面");
        assert!(list.contains(&(48_000, 2)), "设备混音格式是第二选择");
        assert!(list.contains(&(192_000, 2)), "再不行才退到常用档位");
    }

    #[test]
    fn a_multi_channel_source_keeps_its_channel_count_when_falling_back() {
        let list = candidates(Some((96_000, 6)), 48_000, 2);
        assert_eq!(list[1], (48_000, 2), "设备混音格式原样排第二");
        assert!(list.contains(&(48_000, 6)), "退让时也要试 6 声道");
    }

    #[test]
    fn an_unknown_device_still_yields_candidates() {
        let list = candidates(None, 48_000, 2);
        assert_eq!(list[0], (48_000, 2));
        assert!(!list.is_empty());
        // 去重：同一个口径不该重复出现
        let mut seen = list.clone();
        seen.dedup();
        assert_eq!(seen.len(), list.len());
    }

    #[test]
    fn bit_perfect_only_when_both_rate_and_channels_match() {
        let format = ExclusiveFormat {
            sample_rate: 44_100,
            channels: 2,
            samples: PcmFormat::F32,
            block_align: 8,
        };
        assert!(format.is_bit_perfect_for(44_100, 2));
        assert!(!format.is_bit_perfect_for(48_000, 2));
        assert!(!format.is_bit_perfect_for(44_100, 6));
        assert_eq!(format.describe(), "32-bit float");
    }
}
