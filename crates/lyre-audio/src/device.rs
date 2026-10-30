//! 输出设备枚举与稳定标识解析。
//! `id` 是 cpal 各平台的原生稳定标识（Windows 为 WASAPI 的 endpoint ID、macOS 为 `CoreAudio`
//! 的 DeviceUID、Linux 为 ALSA 的 PCM 名），可以落盘；友好名随驱动更新或系统语言变化，只能显示。

use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

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

/// cpal 0.18 的 WASAPI 枚举器是进程级 `OnceLock`，且绑定首次调用线程的 STA：
/// 那条线程退出后指针就悬空（RustAudio/cpal#1302）。设备调用固定在这条长生命周期线程上。
struct DeviceHost {
    tx: Option<Sender<Request>>,
}

enum Request {
    Enumerate(Sender<Vec<AudioDeviceInfo>>),
    DefaultId(Sender<Option<String>>),
    Resolve(String, Sender<Option<cpal::Device>>),
}

static HOST: OnceLock<DeviceHost> = OnceLock::new();

fn host() -> &'static DeviceHost {
    HOST.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let worker_tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("lyre-device-host".to_owned())
            .spawn(move || {
                while let Ok(request) = rx.recv() {
                    match request {
                        Request::Enumerate(reply) => {
                            let _ = reply.send(enumerate_now());
                        }
                        Request::DefaultId(reply) => {
                            let _ = reply.send(default_id_now());
                        }
                        Request::Resolve(id, reply) => {
                            let _ = reply.send(resolve_now(&id));
                        }
                    }
                }
            })
            .is_ok();
        DeviceHost {
            tx: spawned.then_some(worker_tx),
        }
    })
}

fn enumerate_now() -> Vec<AudioDeviceInfo> {
    let host = cpal::default_host();
    // 先取默认设备：cpal 借此在当前线程初始化 COM，再枚举就不会踩到未初始化的公寓。
    let default_id = host
        .default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());

    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };

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

fn default_id_now() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string())
}

fn resolve_now(id: &str) -> Option<cpal::Device> {
    let parsed = id.parse::<cpal::DeviceId>().ok()?;
    let host = cpal::default_host();
    // 枚举器归 device-host 线程所有且一直活着；这里只需保证当前线程的 COM 已初始化。
    let _ = host.default_output_device();
    host.device_by_id(&parsed)
}

/// 枚举全部输出设备。ID 与名称来自同一次枚举，避免两者跨枚举错配。同名设备只保留第一个。
pub fn enumerate_output_devices() -> Vec<AudioDeviceInfo> {
    let (reply, response) = mpsc::channel();
    if host()
        .tx
        .as_ref()
        .is_some_and(|tx| tx.send(Request::Enumerate(reply)).is_ok())
        && let Ok(devices) = response.recv()
    {
        return devices;
    }
    enumerate_now()
}

/// 把落盘标识解析回 cpal 设备。标识过期、设备已拔出或格式不认识时返回 `None`，调用方回退默认设备。
pub fn resolve_device(id: &str) -> Option<cpal::Device> {
    let (reply, response) = mpsc::channel();
    if host()
        .tx
        .as_ref()
        .is_some_and(|tx| tx.send(Request::Resolve(id.to_owned(), reply)).is_ok())
        && let Ok(device) = response.recv()
    {
        return device;
    }
    resolve_now(id)
}

/// 标识对应的友好名，用于切换后回显。
pub fn device_name(id: &str) -> Option<String> {
    enumerate_output_devices()
        .into_iter()
        .find(|device| device.id == id)
        .map(|device| device.name)
}

pub fn default_device_id() -> Option<String> {
    let (reply, response) = mpsc::channel();
    if host()
        .tx
        .as_ref()
        .is_some_and(|tx| tx.send(Request::DefaultId(reply)).is_ok())
        && let Ok(id) = response.recv()
    {
        return id;
    }
    default_id_now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_access_survives_short_lived_threads() {
        for _ in 0..4 {
            std::thread::spawn(|| {
                for _ in 0..5 {
                    let _ = enumerate_output_devices();
                    let _ = default_device_id();
                }
            })
            .join()
            .expect("线程不应 panic");
        }
    }
}
