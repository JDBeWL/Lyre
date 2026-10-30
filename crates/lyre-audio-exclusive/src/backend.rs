//! 独占输出后端：`lyre_audio::AudioBackend` 的第二个实现。
//! 与共享后端共用解码链，末端不同：采样经环缓冲由渲染线程独占写入设备，绕开 rodio 的 mixer。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use lyre_audio::decode::{
    ChannelConvertSource, GainControl, GainRamp, LfeConfig, LockFreeSource, SoftClip, TrackDecoder,
    WhenEmpty,
};
use lyre_audio::{AudioBackend, AudioError, BackendNotice};
use rodio::{ChannelCount, SampleRate, Source};

use crate::device::{ExclusiveFormat, OpenStream, endpoint_of, open_stream};
use crate::error::{ExclusiveError, Result};
use crate::push::{self, PushThread};
use crate::render::{self, RenderCommand, RenderResponse, RenderShared};
use crate::resample::RubatoResampleSource;
use crate::ring::SampleRing;

/// 等协商回执的最长时间。设备被别的程序占着时初始化要重试，但也不能无限等。
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(5);
const FADE: Duration = Duration::from_millis(60);
/// 流失联后的首次重试间隔；之后按倍数退避，避免在坏设备上来回撞。
const RETRY_INITIAL: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(2);

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
    /// 当前是否有一条能用的流。失联或协商失败时置假，`load` 会据此重新协商。
    stream_alive: bool,
    /// 当前曲目的声源口径，失联重建时按它重新协商。
    preferred: Option<(u32, u16)>,
    /// 流失联后的待重建状态；`None` 表示链路正常。
    loss: Option<LostStream>,
    /// 需要上浮给音频线程的异步通知（中断 / 恢复）。
    notices: VecDeque<BackendNotice>,
    current: Option<Loaded>,
    volume: f32,
    stopped: bool,
}

/// 一次待重建的失联状态。
struct LostStream {
    message: String,
    attempts: u32,
    next_retry: Instant,
    /// 中断已经上报过，进程里每拍不要重复刷。
    reported: bool,
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
        Self::open_with(device_id, Box::new(open_stream))
    }

    fn open_with(device_id: Option<&str>, open: OpenStream) -> Result<Self> {
        let (ring, _writer, reader) = SampleRing::new();
        let (commands, command_rx) = bounded::<RenderCommand>(32);
        let (response_tx, responses) = bounded::<RenderResponse>(8);
        let shared = RenderShared::new();

        let thread_shared = shared.clone();
        let render_thread = std::thread::Builder::new()
            .name("lyre-exclusive-render".to_owned())
            .spawn(move || render::run(command_rx, response_tx, reader, thread_shared, open))
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
            stream_alive: false,
            preferred: None,
            loss: None,
            notices: VecDeque::new(),
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

    /// 累计欠载采样数：环里没货、只能拿静音顶上设备的那部分。
    /// 涨了就说明推送线程（解码、建链、换手）跟不上渲染线程，听感是断音或卡顿。
    pub fn underrun_samples(&self) -> u64 {
        self.shared.starved.load(Ordering::Relaxed)
    }

    /// 独占流起播过几次。同一台设备上连续放两首时这个数**不该**涨：
    /// 涨了就说明中间的流被停掉又起了，接缝处会有一声卡顿。
    pub fn stream_starts(&self) -> u64 {
        self.shared.stream_starts.load(Ordering::Relaxed)
    }

    /// 让渲染线程协商一次。`preferred` 是声源口径，设备能接受就是位完美直出。
    fn configure(&mut self, device: Option<String>, preferred: Option<(u32, u16)>) -> Result<()> {
        self.commands
            .send(RenderCommand::Configure { device, preferred })
            .map_err(|_| ExclusiveError::Cancelled)?;

        let deadline = Instant::now() + CONFIGURE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // 清掉可能滞留的旧回执，否则会错配到下一条命令上
                while self.responses.try_recv().is_ok() {}
                self.stream_alive = false;
                return Err(ExclusiveError::Initialize("独占初始化超时".to_owned()));
            }

            match self.responses.recv_timeout(remaining) {
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
                    self.stream_alive = true;
                    return Ok(());
                }
                Ok(RenderResponse::Failed(message)) => {
                    self.stream_alive = false;
                    return Err(ExclusiveError::Unsupported(message));
                }
                // 老流在协商期间报的故障：记下来，继续等这次协商的回执
                Ok(RenderResponse::DeviceLost { message }) => self.note_lost(message),
                Err(RecvTimeoutError::Timeout) => {
                    while self.responses.try_recv().is_ok() {}
                    self.stream_alive = false;
                    return Err(ExclusiveError::Initialize("独占初始化超时".to_owned()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.stream_alive = false;
                    return Err(ExclusiveError::Cancelled);
                }
            }
        }
    }

    /// 保证当前流的口径能接下这一首。现成口径就能位完美直出时不动设备 —— 重开流要几百毫秒。
    fn ensure_format(&mut self, preferred: (u32, u16)) -> Result<()> {
        if self.stream_alive
            && self
                .format
                .is_some_and(|format| format.is_bit_perfect_for(preferred.0, preferred.1))
        {
            return Ok(());
        }
        let device = self.device_id.clone();
        self.configure(device, Some(preferred))?;
        self.loss = None;
        Ok(())
    }

    /// 当前独占流是否已经落在目标设备上。
    /// 跟随默认时不能只看标识：标识没变但流可能还停在旧设备上，要拿此刻默认设备比。
    fn stream_is_on(&self, wanted: Option<&str>) -> bool {
        if !self.stream_alive {
            return false;
        }
        let Some(opened) = self.opened_endpoint.as_deref() else {
            return false;
        };
        match wanted {
            Some(id) => endpoint_of(id) == opened,
            None => lyre_audio::device::default_device_id()
                .is_some_and(|current| endpoint_of(&current) == opened),
        }
    }

    /// 先备好整条链（换采样率、按 `start` 跳转、预解码），再一次性换手：停旧推送线程、作废环、起新线程。
    /// 顺序反了，建链耗时全变成断音。
    fn rebuild(
        &mut self,
        mut decoder: TrackDecoder,
        path: &Path,
        start: Duration,
        autoplay: bool,
    ) -> Result<Duration> {
        let format = self
            .format
            .ok_or_else(|| ExclusiveError::Initialize("独占格式尚未协商".to_owned()))?;

        let duration = decoder.total_duration().unwrap_or(Duration::ZERO);
        if !start.is_zero() {
            decoder.seek(start)?;
        }
        let source_rate = decoder.sample_rate();

        // 推送线程不在实时线程上：解码没跟上时**等**真样本，别让它把补出来的静音
        // 当内容灌进环 —— 那会在每首开头留下一段断断续续的沉默
        let source = LockFreeSource::new(decoder, WhenEmpty::Wait)?;
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

        // 换手：旧推送线程先收工，再作废环里剩下的样本 —— 反过来的话旧线程会在作废之后
        // 又灌进一批。它手里的写入端带着旧世代，作废之后一律写不进东西。
        self.stop_push();
        self.ring.invalidate();
        // 换一代推送线程。完工标志带着世代号，这里不必（也不能）去清零：清零是异步生效的，
        // 新线程可能已经把整首写完并置上了标志，渲染线程这才处理到那条清零命令。
        let generation = self.shared.begin_producer();
        // `written` 必须在此同步归零，不能只等渲染线程处理 `Flush`。
        // 下面马上把 `base` 改成 `start`，旧计数会让 `position()` 多算一段：进度条先虚高再回落。
        self.shared.written.store(0, Ordering::SeqCst);
        let _ = self.commands.send(RenderCommand::Flush);

        self.push = Some(push::spawn(
            clipped,
            SampleRing::writer(&self.ring),
            format.sample_rate,
            format.channels,
            self.shared.done_flag(),
            generation,
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

    /// 收走渲染线程发来的异步回执。没有 `configure` 在等的那几条是超时后的迟到消息，丢掉。
    fn drain_responses(&mut self) {
        while let Ok(response) = self.responses.try_recv() {
            match response {
                RenderResponse::DeviceLost { message } => self.note_lost(message),
                RenderResponse::Configured { .. } | RenderResponse::Failed(_) => {}
            }
        }
    }

    /// 记录一次流中断。重复报同一场故障时只更新原因，不重置退避计数。
    fn note_lost(&mut self, message: String) {
        self.stream_alive = false;
        if let Some(loss) = self.loss.as_mut() {
            loss.message = message;
            return;
        }
        log::warn!("独占输出中断：{message}");
        self.loss = Some(LostStream {
            message,
            attempts: 0,
            next_retry: Instant::now() + RETRY_INITIAL,
            reported: false,
        });
    }

    /// 到点就重试重建一次。失败按指数退避，不让音频线程每拍都卡在开流上。
    fn try_recover(&mut self) {
        if self.stream_alive {
            self.loss = None;
            return;
        }
        let Some(loss) = self.loss.as_ref() else {
            return;
        };
        if Instant::now() < loss.next_retry {
            return;
        }

        match self.recover(None) {
            Ok(()) => {
                log::info!("独占输出已恢复");
                self.loss = None;
                if self.current.as_ref().is_some_and(|current| current.playing) {
                    self.notices.push_back(BackendNotice::Resumed);
                }
            }
            Err(err) => {
                if let Some(loss) = self.loss.as_mut() {
                    loss.attempts += 1;
                    let delay = retry_delay(loss.attempts);
                    loss.next_retry = Instant::now() + delay;
                    log::warn!(
                        "独占输出重建失败（第 {} 次，{:?} 后重试）：{err}",
                        loss.attempts,
                        delay
                    );
                }
            }
        }
    }

    /// 按当前曲目重建整条流。`autoplay` 为 `None` 时沿用曲目原本的播放意图。
    fn recover(&mut self, autoplay: Option<bool>) -> Result<()> {
        let preferred = self.preferred;
        let Some(current) = self.current.as_ref() else {
            self.configure(self.device_id.clone(), preferred)?;
            return Ok(());
        };

        let ended = self.shared.finished.load(Ordering::Acquire);
        let path = current.path.clone();
        let position = if ended { current.base } else { self.position() };
        let playing = autoplay.unwrap_or(current.playing && !ended);

        self.configure(self.device_id.clone(), preferred)?;
        let decoder = TrackDecoder::open(&path)?;
        self.rebuild(decoder, &path, position, playing)?;
        Ok(())
    }
}

/// 失联重建的退避间隔：250ms、500ms、1s、2s 封顶。
fn retry_delay(attempts: u32) -> Duration {
    let factor = 1u32 << attempts.min(4);
    (RETRY_INITIAL * factor).min(RETRY_MAX)
}

fn sample_rate(value: u32) -> SampleRate {
    std::num::NonZeroU32::new(value).unwrap_or(SampleRate::MIN)
}

fn channel_count(value: u16) -> ChannelCount {
    std::num::NonZeroU16::new(value).unwrap_or(ChannelCount::MIN)
}

impl AudioBackend for ExclusiveBackend {
    fn load(&mut self, path: &Path, autoplay: bool) -> std::result::Result<Duration, AudioError> {
        // 解码器只打开一次：口径要从它身上看，链路又得用它来建。
        // 分开打开的话，WavPack 这类要通读一遍文件的格式会白读两遍。
        let decoder = TrackDecoder::open(path)?;
        let preferred = (decoder.sample_rate(), decoder.channels());
        self.preferred = Some(preferred);
        self.ensure_format(preferred).map_err(AudioError::from)?;
        self.rebuild(decoder, path, Duration::ZERO, autoplay)
            .map_err(AudioError::from)
    }

    fn play(&mut self) -> std::result::Result<(), AudioError> {
        if self.loss.is_some() {
            // 用户显式播放：不等退避，立刻按当前曲目重建一次
            self.recover(Some(true)).map_err(AudioError::from)?;
            self.loss = None;
            return Ok(());
        }

        let ended = self.shared.finished.load(Ordering::Acquire);
        if self.stopped || self.push.is_none() || ended {
            // 播完之后再按播放键：按当前曲目重建（与共享后端同语义）
            let Some(current) = self.current.as_ref() else {
                return Ok(());
            };
            let (path, start) = (current.path.clone(), current.base);
            let decoder = TrackDecoder::open(&path)?;
            self.rebuild(decoder, &path, start, true)
                .map_err(AudioError::from)?;
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

        // 独占下的 seek 只能重建链路：环里缓存着几秒采样，原地跳既复杂又不精确。
        // 跳转与建链要在旧链路还在播的时候做完，换手的空档才短得听不出。
        let decoder = TrackDecoder::open(&path)?;
        self.rebuild(decoder, &path, target, was_playing)
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

    fn tick(&mut self) {
        self.drain_responses();

        if let Some(loss) = self.loss.as_mut()
            && !loss.reported
        {
            loss.reported = true;
            self.notices
                .push_back(BackendNotice::Lost(AudioError::Exclusive(format!(
                    "独占输出中断：{}",
                    loss.message
                ))));
        }

        self.try_recover();
    }

    fn take_notice(&mut self) -> Option<BackendNotice> {
        self.notices.pop_front()
    }

    fn set_device(&mut self, device: Option<&str>) -> std::result::Result<(), AudioError> {
        let wanted = device.map(str::to_owned);
        // 光比标识不够：跟随默认时默认设备被换掉，标识没变，但流必须重建。
        if wanted == self.device_id && self.stream_is_on(wanted.as_deref()) {
            return Ok(());
        }

        let Some(current) = self.current.as_ref() else {
            // 还没装载任何东西：下次 load 自然按新设备协商
            self.device_id = wanted;
            return Ok(());
        };
        let previous = self.device_id.clone();
        let (path, playing) = (current.path.clone(), current.playing);
        // 从「此刻播到的位置」续上，不能用 `current.base`：base 只在 load / seek 时更新，
        // 拿它重建会把上次手动跳转之后播过的那一段全丢掉，进度直接退回去。
        let start = self.position();

        // 解码器先打开：文件打不开就不该动设备
        let decoder = TrackDecoder::open(&path)?;
        let preferred = Some((decoder.sample_rate(), decoder.channels()));
        self.preferred = preferred;

        match self.configure(wanted.clone(), preferred) {
            Ok(()) => {
                // 新流真的开起来了才提交选择；协商失败时 self.device_id 还留在旧值上
                self.device_id = wanted;
                self.loss = None;
                self.rebuild(decoder, &path, start, playing)
                    .map_err(AudioError::from)
                    .map(|_| ())
            }
            Err(switch_err) => {
                if previous == wanted {
                    // 目标就是旧设备：没有可回滚的别处，交给失联退避或下一次显式操作
                    return Err(AudioError::from(switch_err));
                }
                // 新设备不可用：把旧设备的流重建回来，别让一次失败的切换把声音弄没
                let rollback = match self.configure(previous, preferred) {
                    Ok(()) => self
                        .rebuild(decoder, &path, start, playing)
                        .map_err(AudioError::from)
                        .map(|_| ()),
                    Err(err) => Err(AudioError::from(err)),
                };
                match rollback {
                    Ok(()) => {
                        self.loss = None;
                        Err(AudioError::from(switch_err))
                    }
                    Err(rollback_err) => Err(AudioError::Exclusive(format!(
                        "{switch_err}；回退旧设备也失败：{rollback_err}"
                    ))),
                }
            }
        }
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
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::device::{OpenStream, RenderStream};
    use crate::test_support::{FakeControl, FakeStream, silent_wav};

    fn control(controls: &Arc<Mutex<Vec<Arc<FakeControl>>>>, index: usize) -> Arc<FakeControl> {
        Arc::clone(&controls.lock().expect("controls lock")[index])
    }

    fn wait_position(backend: &ExclusiveBackend, wanted: Duration) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while backend.position() < wanted {
            assert!(
                Instant::now() < deadline,
                "播放位置没有推进到 {wanted:?}，当前 {:?}",
                backend.position()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn exclusive_errors_surface_as_a_single_audio_error_variant() {
        let error = AudioError::from(ExclusiveError::Unsupported("设备不支持".to_owned()));
        assert!(matches!(error, AudioError::Exclusive(_)));
        assert!(
            error.to_string().contains("设备不支持"),
            "原因不能丢：{error}"
        );
    }

    /// 切到一台开不起来的设备时，`device_id` 不能被提前提交，旧设备的流要重建回来继续播。
    #[test]
    fn a_failed_device_switch_rolls_back_to_the_previous_output() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = silent_wav(dir.path(), 44_100 * 5);

        let controls = Arc::new(Mutex::new(Vec::new()));
        let devices = Arc::new(Mutex::new(Vec::new()));
        let open: OpenStream = {
            let controls = Arc::clone(&controls);
            let devices = Arc::clone(&devices);
            Box::new(move |device, _| {
                devices
                    .lock()
                    .expect("devices lock")
                    .push(device.map(str::to_owned));
                if device.is_some_and(|id| id.contains("target-B")) {
                    return Err(ExclusiveError::Unsupported("设备被占用".to_owned()));
                }
                let control = Arc::new(FakeControl::default());
                controls
                    .lock()
                    .expect("controls lock")
                    .push(Arc::clone(&control));
                Ok(Box::new(FakeStream { control }) as Box<dyn RenderStream>)
            })
        };

        let mut backend = ExclusiveBackend::open_with(None, open).expect("默认设备应当能开流");
        backend.load(&path, true).expect("装载应当成功");
        wait_position(&backend, Duration::from_millis(40));
        let before = backend.position();

        let error = backend
            .set_device(Some("wasapi:{0.0.0.00000000}.{target-B}"))
            .expect_err("新设备被占用时必须报错");
        assert!(error.to_string().contains("设备被占用"), "{error}");

        assert_eq!(
            backend.current_device(),
            None,
            "切换失败后选择必须留在旧设备上"
        );
        assert!(backend.stream_alive, "旧流应当已经重建回来");
        assert!(
            backend
                .current
                .as_ref()
                .is_some_and(|current| current.playing),
            "回滚之后仍应保持播放意图"
        );
        assert_eq!(
            *devices.lock().expect("devices lock"),
            vec![
                None,
                Some("wasapi:{0.0.0.00000000}.{target-B}".to_owned()),
                None,
            ],
            "失败后应当按旧设备重新协商"
        );
        assert!(
            backend.describe_output().contains("44100"),
            "回退后口径应当重新协商：{}",
            backend.describe_output()
        );

        let after = backend.position();
        assert!(
            after >= before,
            "回滚不该让进度倒退：{before:?} → {after:?}"
        );
        wait_position(&backend, after + Duration::from_millis(40));
    }

    /// 流中途失效：先上报可恢复错误，再按退避重建；设备回来后自动续播。
    #[test]
    fn a_lost_device_is_retried_with_backoff_and_playback_resumes() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = silent_wav(dir.path(), 44_100 * 10);

        let controls = Arc::new(Mutex::new(Vec::new()));
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let opened = Arc::new(AtomicUsize::new(0));
        let open: OpenStream = {
            let controls = Arc::clone(&controls);
            let attempts = Arc::clone(&attempts);
            let opened = Arc::clone(&opened);
            Box::new(move |_, _| {
                attempts.lock().expect("attempts lock").push(Instant::now());
                // 第 0 次是初始开流；第 1 次恢复撞上「设备仍被占用」；再下一次成功
                if opened.fetch_add(1, Ordering::SeqCst) == 1 {
                    return Err(ExclusiveError::Unsupported("设备被占用".to_owned()));
                }
                let control = Arc::new(FakeControl::default());
                controls
                    .lock()
                    .expect("controls lock")
                    .push(Arc::clone(&control));
                Ok(Box::new(FakeStream { control }) as Box<dyn RenderStream>)
            })
        };

        let mut backend = ExclusiveBackend::open_with(None, open).expect("默认设备应当能开流");
        backend.load(&path, true).expect("装载应当成功");
        wait_position(&backend, Duration::from_millis(30));

        // 拔卡 / 驱动复位：当前流下一次写设备就失败
        control(&controls, 0)
            .fail_write
            .store(true, Ordering::Relaxed);
        let loss_at = Instant::now();

        let mut lost = None;
        while Instant::now() < loss_at + Duration::from_secs(2) {
            backend.tick();
            while let Some(notice) = backend.take_notice() {
                if let BackendNotice::Lost(error) = notice {
                    lost = Some(error);
                }
            }
            if lost.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let lost = lost.expect("流中断必须上报");
        assert!(lost.to_string().contains("独占输出中断"), "{lost}");
        assert!(!backend.stream_alive, "上报中断时流应当已经不可用");
        assert!(!backend.is_finished(), "设备断流不能被当成整首放完");

        let mut resumed = false;
        while Instant::now() < loss_at + Duration::from_secs(3) {
            backend.tick();
            while let Some(notice) = backend.take_notice() {
                if matches!(notice, BackendNotice::Resumed) {
                    resumed = true;
                }
            }
            if resumed {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(resumed, "设备恢复后应当自动续播");

        let times = attempts.lock().expect("attempts lock").clone();
        assert_eq!(times.len(), 3, "应当是初始 + 两次恢复尝试：{times:?}");
        let first_retry = times[1].duration_since(times[0]);
        assert!(
            first_retry >= Duration::from_millis(200),
            "首次重试必须等退避，不能每拍撞一次：{first_retry:?}"
        );
        let second_retry = times[2].duration_since(times[1]);
        assert!(
            second_retry >= Duration::from_millis(400),
            "失败后应当继续加大间隔：{second_retry:?}"
        );

        let position = backend.position();
        wait_position(&backend, position + Duration::from_millis(40));
    }
}
