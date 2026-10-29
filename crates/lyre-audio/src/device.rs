//! 输出设备枚举与稳定标识解析。
//! `id` 是 cpal 各平台的原生稳定标识（Windows 为 WASAPI 的 endpoint ID、macOS 为 `CoreAudio`
//! 的 DeviceUID、Linux 为 ALSA 的 PCM 名），可以落盘；友好名随驱动更新或系统语言变化，只能显示。

use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait};

/// 一个可用的输出设备。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// cpal 的 WASAPI 后端把 `DEVPKEY_Device_FriendlyName` 放在 `extended()` 的第一项，
/// `description().name()` 只是设备描述（"Speakers" 而非 "Speakers (Realtek...)"）。
fn friendly_name(device: &cpal::Device) -> Option<String> {
    let description = device.description().ok()?;
    description
        .extended()
        .next()
        .map(str::to_owned)
        .or_else(|| Some(description.name().to_owned()))
}

/// 枚举全部输出设备。ID 与名称来自同一次枚举，避免两者跨枚举错配。同名设备只保留第一个。
pub fn enumerate_output_devices() -> Vec<AudioDeviceInfo> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };

    let default_id = host
        .default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());

    let mut found: Vec<AudioDeviceInfo> = Vec::new();
    for device in devices {
        let (Ok(id), Some(name)) = (device.id(), friendly_name(&device)) else {
            continue;
        };
        if found.iter().any(|known| known.name == name) {
            continue;
        }
        let id = id.to_string();
        let is_default = default_id.as_deref() == Some(id.as_str());
        found.push(AudioDeviceInfo {
            id,
            name,
            is_default,
        });
    }
    found
}

/// 把落盘标识解析回 cpal 设备。标识过期、设备已拔出或格式不认识时返回 `None`，调用方回退默认设备。
pub fn resolve_device(id: &str) -> Option<cpal::Device> {
    let parsed = id.parse::<cpal::DeviceId>().ok()?;
    cpal::default_host().device_by_id(&parsed)
}

/// 标识对应的友好名，用于切换后回显。
pub fn device_name(id: &str) -> Option<String> {
    enumerate_output_devices()
        .into_iter()
        .find(|device| device.id == id)
        .map(|device| device.name)
}

/// 当前系统默认输出设备的标识。
pub fn default_device_id() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string())
}
