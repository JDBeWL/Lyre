//! 独占输出后端：`lyre_audio::AudioBackend` 的第二个实现。
//!
//! 与共享后端共用同一套解码链（`lyre_audio::decode` 里的那几个适配器），差别只在末端：
//! 共享后端把链交给 rodio 的 mixer，这里则把链的输出灌进采样环，由渲染线程独占写进设备。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use lyre_audio::decode::{
    ChannelConvertSource, GainControl, GainRamp, LfeConfig, LockFreeSource, SoftClip,
    SymphoniaDecoder,
};
use lyre_audio::{AudioBackend, AudioError};
use rodio::{ChannelCount, SampleRate, Source};

use crate::device::{ExclusiveFormat, endpoint_of};
use crate::error::{ExclusiveError, Result};
use crate::push::{self, PushThread};
use crate::render::{self, RenderCommand, RenderResponse, RenderShared};
use crate::resample::RubatoResampleSource;
use crate::ring::SampleRing;

/// 等协商回执的最长时间。设备被别的程序占着时初始化要重试，但也不能无限等。
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(5);
/// 链路上的淡入淡出时长。
const FADE: Duration = Duration::from_millis(60);

pub struct ExclusiveBackend {
    ring: Arc<SampleRing>,
    commands: Sender<RenderCommand>,
    responses: Receiver<RenderResponse>,
    render_thread: Option<JoinHandle<()>>,
    shared: RenderShared,
    push: Option<PushThread>,
    format: Option<ExclusiveFormat>,
    /// 当前流真正落在哪台设备上（WASAPI endpoint）。选定设备不可用时开流会退回默认设备，
    /// 所以它与 `device_id` 并不总是一回事。
    opened_endpoint: Option<String>,
    device_name: String,
    /// 用户选定的设备稳定标识；`None` 跟随系统默认。
    device_id: Option<String>,
    current: Option<Loaded>,
    volume: f32,
    stopped: bool,
}

struct Loaded {
    path: PathBuf,
    duration: Duration,
    /// 本次链路的起点。渲染线程的 `written` 从 0 起算，位置要把它加上。
    base: Duration,
    playing: bool,
}

impl ExclusiveBackend {
    /// 打开独占后端。会立刻协商一次格式 —— 协商不过就说明这台设备用不了独占，
    /// 由上层回退到共享后端。
    pub fn open(device_id: Option<&str>) -> Result<Self> {
        let (ring, _writer, reader) = SampleRing::new();
        let (commands, command_rx) = bounded::<RenderCommand>(32);
        let (response_tx, responses) = bounded::<RenderResponse>(8);
        let shared = RenderShared::new();

        let thread_shared = shared.clone();
        let render_thread = std::thread::Builder::new()
            .name("lyre-exclusive-render".to_owned())
            .spawn(move || render::run(command_rx, response_tx, reader, thread_shared))
            .map_err(ExclusiveError::ThreadSpawn)?;

        let mut backend = Self {
            ring,
            commands,
            responses,
            render_thread: Some(render_thread),
            shared,
            push: None,
            format: None,
            opened_endpoint: None,
            device_name: String::new(),
            device_id: device_id.map(str::to_owned),
            current: None,
            volume: 1.0,
            stopped: false,
        };

        if let Err(err) = backend.configure(device_id.map(str::to_owned), None) {
            backend.shutdown();
            return Err(err);
        }
        Ok(backend)
    }

    /// 当前协商到的格式，UI 用它显示「是否位完美」。
    pub fn format(&self) -> Option<ExclusiveFormat> {
        self.format
    }

    /// 当前独占流所在设备的友好名。
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// 让渲染线程协商一次。`preferred` 是声源口径，设备能接受就是位完美直出。
    fn configure(&mut self, device: Option<String>, preferred: Option<(u32, u16)>) -> Result<()> {
        self.commands
            .send(RenderCommand::Configure { device, preferred })
            .map_err(|_| ExclusiveError::Cancelled)?;

        match self.responses.recv_timeout(CONFIGURE_TIMEOUT) {
            Ok(RenderResponse::Configured {
                format,
                device_name,
                device_endpoint,
            }) => {
                log::info!(
                    "独占输出：{} @ {} Hz / {} ch / {}",
                    device_name,
                    format.sample_rate,
                    format.channels,
                    format.describe()
                );
                self.format = Some(format);
                self.device_name = device_name;
                self.opened_endpoint = Some(device_endpoint);
                Ok(())
            }
            Ok(RenderResponse::Failed(message)) => Err(ExclusiveError::Unsupported(message)),
            Err(RecvTimeoutError::Timeout) => {
                // 清掉可能滞留的旧回执，否则会错配到下一条命令上
                while self.responses.try_recv().is_ok() {}
                Err(ExclusiveError::Initialize("独占初始化超时".to_owned()))
            }
            Err(RecvTimeoutError::Disconnected) => Err(ExclusiveError::Cancelled),
        }
    }

    /// 保证当前流的口径能接下这一首。现成口径就能位完美直出时不动设备 ——
    /// 重开流要几百毫秒，而曲库里 44.1k 与 48k 各占一半，能省则省。
    fn ensure_format(&mut self, preferred: (u32, u16)) -> Result<()> {
        if self
            .format
            .is_some_and(|format| format.is_bit_perfect_for(preferred.0, preferred.1))
        {
            return Ok(());
        }
        let device = self.device_id.clone();
        self.configure(device, Some(preferred))
    }

    /// 当前独占流是否已经落在目标设备上。
    ///
    /// 跟随默认时不能只看标识：默认设备会被插拔换掉，那时标识没变但流还停在旧设备上，
    /// 必须重建才跟得上。所以这里拿此刻的默认设备来比。
    fn stream_is_on(&self, wanted: Option<&str>) -> bool {
        let Some(opened) = self.opened_endpoint.as_deref() else {
            return false;
        };
        match wanted {
            Some(id) => endpoint_of(id) == opened,
            None => lyre_audio::device::default_device_id()
                .is_some_and(|current| endpoint_of(&current) == opened),
        }
    }

    /// 建一条链并把它的输出灌进环。
    fn rebuild(&mut self, path: &Path, start: Duration, autoplay: bool) -> Result<Duration> {
        // 旧线程先收工，再作废环里剩下的样本：反过来的话旧线程会在作废之后又灌进一批。
        self.stop_push();
        self.ring.invalidate();
        self.shared.producer_done.store(false, Ordering::SeqCst);
        // `written` 必须在这里同步归零，不能只靠渲染线程处理 `Flush`：下面马上就把
        // `base` 改成 `start`，旧计数还在的话 `position()` 会立刻多算出一整段
        // ——表现为切换瞬间进度条先虚高一截再回落。
        self.shared.written.store(0, Ordering::SeqCst);
        let _ = self.commands.send(RenderCommand::Flush);

        let format = self
            .format
            .ok_or_else(|| ExclusiveError::Initialize("独占格式尚未协商".to_owned()))?;

        let mut decoder = SymphoniaDecoder::new(path)?;
        let duration = decoder.total_duration().unwrap_or(Duration::ZERO);
        if !start.is_zero() {
            decoder.seek(start)?;
        }
        let source_rate = decoder.sample_rate();

        let source = LockFreeSource::new(decoder)?;
        // 采样率一致就整段跳过重采样 —— 这正是位完美那条路径
        let resampled: Box<dyn Source<Item = f32> + Send> = if source_rate == format.sample_rate {
            Box::new(source)
        } else {
            Box::new(RubatoResampleSource::new(
                source,
                sample_rate(format.sample_rate),
            )?)
        };

        let converted = ChannelConvertSource::new(
            resampled,
            channel_count(format.channels),
            LfeConfig::default(),
        );
        let control = Arc::new(GainControl::new());
        let ramped = GainRamp::new(
            converted,
            FADE,
            format.sample_rate,
            format.channels,
            Arc::clone(&control),
        );
        let clipped = SoftClip::new(ramped);

        self.push = Some(push::spawn(
            clipped,
            SampleRing::writer(&self.ring),
            format.sample_rate,
            format.channels,
            Arc::clone(&self.shared.producer_done),
        )?);

        self.current = Some(Loaded {
            path: path.to_path_buf(),
            duration,
            base: start,
            playing: autoplay,
        });
        self.stopped = false;
        if autoplay {
            let _ = self.commands.send(RenderCommand::Start);
        }
        Ok(duration)
    }

    fn stop_push(&mut self) {
        if let Some(push) = self.push.take() {
            push.join();
        }
    }

    fn shutdown(&mut self) {
        let _ = self.commands.send(RenderCommand::Shutdown);
        self.stop_push();
        if let Some(thread) = self.render_thread.take() {
            let _ = thread.join();
        }
    }
}

/// 采样率/声道数转成 rodio 的 `NonZero` 包装。
fn sample_rate(value: u32) -> SampleRate {
    std::num::NonZeroU32::new(value).unwrap_or(SampleRate::MIN)
}

fn channel_count(value: u16) -> ChannelCount {
    std::num::NonZeroU16::new(value).unwrap_or(ChannelCount::MIN)
}

/// 只读文件头拿采样率与声道数，用来挑独占格式。
fn probe(path: &Path) -> Result<(u32, u16)> {
    let decoder = SymphoniaDecoder::new(path)?;
    Ok((decoder.sample_rate(), decoder.channels()))
}

impl AudioBackend for ExclusiveBackend {
    fn load(&mut self, path: &Path, autoplay: bool) -> std::result::Result<Duration, AudioError> {
        let preferred = probe(path).map_err(AudioError::from)?;
        self.ensure_format(preferred).map_err(AudioError::from)?;
        self.rebuild(path, Duration::ZERO, autoplay)
            .map_err(AudioError::from)
    }

    fn play(&mut self) -> std::result::Result<(), AudioError> {
        let ended = self.shared.finished.load(Ordering::Acquire);
        if self.stopped || self.push.is_none() || ended {
            // 播完之后再按播放键：按当前曲目重建（与共享后端同语义）
            let Some(current) = self.current.as_ref() else {
                return Ok(());
            };
            let (path, start) = (current.path.clone(), current.base);
            self.rebuild(&path, start, true).map_err(AudioError::from)?;
            return Ok(());
        }

        let _ = self.commands.send(RenderCommand::Start);
        self.stopped = false;
        if let Some(current) = self.current.as_mut() {
            current.playing = true;
        }
        Ok(())
    }

    fn pause(&mut self) {
        let _ = self.commands.send(RenderCommand::Pause);
        if let Some(current) = self.current.as_mut() {
            current.playing = false;
        }
    }

    fn stop(&mut self) {
        let _ = self.commands.send(RenderCommand::Stop);
        self.stop_push();
        if let Some(current) = self.current.as_mut() {
            current.base = Duration::ZERO;
            current.playing = false;
        }
        self.stopped = true;
    }

    fn seek(&mut self, position: Duration) -> std::result::Result<(), AudioError> {
        let Some(current) = self.current.as_ref() else {
            return Ok(());
        };
        let (path, duration, was_playing) =
            (current.path.clone(), current.duration, current.playing);
        let target = if duration > Duration::ZERO {
            position.min(duration)
        } else {
            position
        };

        // 独占下的 seek 只能重建链路：环里缓存着几秒采样，原地跳既复杂又不精确
        self.rebuild(&path, target, was_playing)
            .map_err(AudioError::from)
            .map(|_| ())
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        let _ = self.commands.send(RenderCommand::SetVolume(self.volume));
    }

    fn position(&self) -> Duration {
        let Some(current) = self.current.as_ref() else {
            return Duration::ZERO;
        };
        let Some(format) = self.format else {
            return current.base;
        };

        let per_second = u64::from(format.sample_rate) * u64::from(format.channels.max(1));
        if per_second == 0 {
            return current.base;
        }
        // 只算已经交给设备的那部分：环里还没写进去的不算播过
        let seconds = self.shared.written.load(Ordering::Acquire) as f64 / per_second as f64;
        let position = current.base + Duration::from_secs_f64(seconds);

        if current.duration > Duration::ZERO {
            position.min(current.duration)
        } else {
            position
        }
    }

    fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }

    fn set_device(&mut self, device: Option<&str>) -> std::result::Result<(), AudioError> {
        let wanted = device.map(str::to_owned);
        // 光比标识是不够的：跟随默认时默认设备被换掉之后标识没变，但流必须重建。
        // 只比标识会把这种情况当成「没变」直接返回，于是界面已经显示新默认设备、声音却还在旧的上面。
        if wanted == self.device_id && self.stream_is_on(wanted.as_deref()) {
            return Ok(());
        }
        self.device_id = wanted;

        let Some(current) = self.current.as_ref() else {
            // 还没装载任何东西：下次 load 自然按新设备协商
            return Ok(());
        };
        let (path, playing) = (current.path.clone(), current.playing);
        // 从「此刻播到的位置」续上，不能用 `current.base`：base 只在 load / seek 时更新，
        // 拿它重建会把上次手动跳转之后播过的那一段全丢掉，进度直接退回去。
        let start = self.position();
        let preferred = probe(&path).map_err(AudioError::from)?;
        self.configure(self.device_id.clone(), Some(preferred))
            .map_err(AudioError::from)?;
        self.rebuild(&path, start, playing)
            .map_err(AudioError::from)
            .map(|_| ())
    }

    fn current_device(&self) -> Option<String> {
        self.device_id.clone()
    }

    fn describe_output(&self) -> String {
        match self.format {
            Some(format) => format!(
                "{} Hz / {} 声道 / {}",
                format.sample_rate,
                format.channels,
                format.describe()
            ),
            None => String::new(),
        }
    }
}

impl Drop for ExclusiveBackend {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_errors_surface_as_a_single_audio_error_variant() {
        let error = AudioError::from(ExclusiveError::Unsupported("设备不支持".to_owned()));
        assert!(matches!(error, AudioError::Exclusive(_)));
        assert!(
            error.to_string().contains("设备不支持"),
            "原因不能丢：{error}"
        );
    }
}
