//! 独占输出的渲染线程：命令处理、事件驱动的样本搬运、音量与淡入淡出。
//!
//! 这条线程之外没有任何人碰过 `AudioClient`：COM 对象在哪个线程上建的就只在哪个线程上用，
//! 控制线程只经命令通道与它打交道。
//!
//! 淡入淡出按**挂钟时间**推进，不按已处理的帧数 —— 设备停转时一帧都处理不了，
//! 按帧计数会让 Stop/Pause 的淡出动作永远走不完，状态就卡在那了。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use lyre_audio_simd::PcmFormat;

use crate::device::{ExclusiveFormat, ExclusiveStream};
use crate::ring::RingReader;

/// 等待设备事件的最长时间。命令也靠这一拍轮询，所以不能取太长。
const EVENT_TIMEOUT_MS: u32 = 5;
/// 没有流可写时的轮询间隔。
const IDLE_SLEEP: Duration = Duration::from_millis(10);
/// 单轮最多搬运的采样数。约 170ms @48k 立体声，远大于任何设备周期，
/// 一次分配到位之后循环里不再扩容。
const MAX_CHUNK_SAMPLES: usize = 16_384;
/// 淡入淡出时长。够抹平接缝又听不出爬升。
pub const FADE: Duration = Duration::from_millis(30);
/// 欠载日志的最小间隔，避免异常时刷屏。
const UNDERRUN_REPORT: Duration = Duration::from_secs(1);
/// 环排空之后再写这么久静音才停流，让设备缓冲里最后那点真实音频播完。
const DRAIN_LINGER: Duration = Duration::from_millis(60);

/// 送给渲染线程的命令。只有协商需要回执，其余是单向的 —— 通道本身保序。
#[derive(Clone, Debug)]
pub enum RenderCommand {
    /// 按指定设备与声源口径协商（重新）打开独占流。
    Configure {
        device: Option<String>,
        preferred: Option<(u32, u16)>,
    },
    Start,
    Pause,
    Stop,
    SetVolume(f32),
    /// 环里残留的样本作废，播放位置归零（切歌、seek）。
    Flush,
    Shutdown,
}

/// 渲染线程的回执。
#[derive(Clone, Debug)]
pub enum RenderResponse {
    Configured {
        format: ExclusiveFormat,
        device_name: String,
        /// 流真正落在哪台设备上（可能因选定设备已拔出而退回默认设备）。
        device_endpoint: String,
    },
    Failed(String),
}

/// 渲染线程与控制线程共享的几个标志。
#[derive(Clone)]
pub struct RenderShared {
    /// 推送线程已把整首写完。
    pub producer_done: Arc<AtomicBool>,
    /// 已写进设备的样本数，控制线程据此算播放位置。
    pub written: Arc<AtomicU64>,
    /// 整首自然放完。
    pub finished: Arc<AtomicBool>,
}

impl RenderShared {
    pub fn new() -> Self {
        Self {
            producer_done: Arc::new(AtomicBool::new(false)),
            written: Arc::new(AtomicU64::new(0)),
            finished: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Default for RenderShared {
    fn default() -> Self {
        Self::new()
    }
}

/// 渲染线程主体。返回时独占流已经关掉。
///
/// 两个通道按值收：线程主体要长期持有它们，改成引用反而要求调用方一直保活。
#[allow(clippy::needless_pass_by_value)]
pub fn run(
    commands: Receiver<RenderCommand>,
    responses: Sender<RenderResponse>,
    reader: RingReader,
    shared: RenderShared,
) {
    let mut renderer = Renderer {
        reader,
        stream: None,
        format: None,
        volume: 1.0,
        fade: Fade::idle(),
        playing: false,
        drain_deadline: None,
        shared,
        samples: Vec::with_capacity(MAX_CHUNK_SAMPLES),
        bytes: Vec::with_capacity(MAX_CHUNK_SAMPLES * 4),
        underrun: Underrun::new(),
    };

    log::info!("独占输出渲染线程已启动");

    loop {
        let mut shutting_down = false;
        while let Ok(command) = commands.try_recv() {
            match command {
                RenderCommand::Shutdown => {
                    shutting_down = true;
                    break;
                }
                other => renderer.handle(other, &responses),
            }
        }
        if shutting_down {
            break;
        }

        renderer.advance_fade();

        if renderer.playing {
            renderer.pump();
        } else {
            thread::sleep(IDLE_SLEEP);
        }
    }

    renderer.shutdown();
    log::info!("独占输出渲染线程已退出");
}

struct Renderer {
    reader: RingReader,
    stream: Option<ExclusiveStream>,
    format: Option<ExclusiveFormat>,
    volume: f32,
    fade: Fade,
    playing: bool,
    /// 环已排空、正在等设备缓冲播完的时刻。
    drain_deadline: Option<Instant>,
    shared: RenderShared,
    samples: Vec<f32>,
    bytes: Vec<u8>,
    underrun: Underrun,
}

impl Renderer {
    fn handle(&mut self, command: RenderCommand, responses: &Sender<RenderResponse>) {
        match command {
            RenderCommand::Shutdown => {}

            RenderCommand::Configure { device, preferred } => {
                // 独占是排他的：不先把手上的流放掉，新的那条永远开不起来 —— 设备已经被自己占住了。
                // 顺序错了表现为「初始化超时」，而不是什么好懂的报错。
                self.stop_stream();
                self.stream = None;

                let opened = ExclusiveStream::open(device.as_deref(), preferred);
                let reply = match opened {
                    Ok(stream) => {
                        let format = stream.format;
                        let device_name = stream.device_name.clone();
                        let device_endpoint = stream.device_endpoint.clone();
                        self.format = Some(format);
                        self.stream = Some(stream);
                        // 新设备上重新起播时从头淡入，别带着上一台的增益
                        self.fade = Fade::idle();
                        self.playing = false;
                        RenderResponse::Configured {
                            format,
                            device_name,
                            device_endpoint,
                        }
                    }
                    Err(err) => RenderResponse::Failed(err.to_string()),
                };
                let _ = responses.send(reply);
            }

            RenderCommand::Start => self.start(),
            RenderCommand::Pause => self.fade.to(0.0, FadeThen::Pause),
            RenderCommand::Stop => self.fade.to(0.0, FadeThen::Stop),
            RenderCommand::SetVolume(volume) => self.volume = volume.clamp(0.0, 1.0),

            RenderCommand::Flush => {
                // 环的世代已由控制线程作废，读取端下一次读会自己丢掉残留
                self.shared.written.store(0, Ordering::SeqCst);
                self.shared.finished.store(false, Ordering::SeqCst);
                self.shared.producer_done.store(false, Ordering::SeqCst);
                self.fade = Fade::idle();
                self.drain_deadline = None;
                self.underrun.reset();
            }
        }
    }

    fn start(&mut self) {
        let Some(stream) = self.stream.as_ref() else {
            return; // 没协商成功过，没有可播的流
        };
        if self.playing {
            return;
        }
        if stream.client.start_stream().is_err() {
            return;
        }
        self.playing = true;
        self.drain_deadline = None;
        // 从当前增益淡入到 1.0（暂停后恢复就从 0 爬起来）
        self.fade.to(1.0, FadeThen::Nothing);
    }

    /// 按挂钟时间推进淡入淡出，走完就执行挂起的动作。
    fn advance_fade(&mut self) {
        match self.fade.advance() {
            FadeThen::Nothing => {}
            FadeThen::Pause => {
                self.stop_stream();
                self.fade.jump(0.0);
            }
            FadeThen::Stop => {
                self.stop_stream();
                self.shared.written.store(0, Ordering::SeqCst);
                self.fade.jump(1.0);
            }
        }
    }

    fn stop_stream(&mut self) {
        if let Some(stream) = self.stream.as_ref() {
            let _ = stream.client.stop_stream();
        }
        self.playing = false;
    }

    /// 等一次设备事件，把缓冲里的空间填满。
    fn pump(&mut self) {
        let Some(stream) = self.stream.as_ref() else {
            return;
        };
        let Some(format) = self.format else {
            return;
        };
        if stream.event.wait_for_event(EVENT_TIMEOUT_MS).is_err() {
            return;
        }
        let Ok(available) = stream.client.get_available_space_in_frames() else {
            return;
        };

        let channels = usize::from(format.channels.max(1));
        let frames = (available as usize).min(MAX_CHUNK_SAMPLES / channels);
        if frames == 0 {
            return;
        }
        let needed = frames * channels;

        self.samples.clear();
        self.samples.resize(needed, 0.0);
        let filled = self.reader.read(&mut self.samples);
        if filled < needed {
            // 环里暂时没货：剩下的保持静音（欠载），绝不能在这里阻塞实时线程
            self.underrun.record(needed - filled);
        }

        let gain = self.volume * self.fade.factor;
        if gain != 1.0 {
            for sample in &mut self.samples {
                *sample *= gain;
            }
        }

        let payload: &[u8] = if format.samples == PcmFormat::F32 {
            // 零拷贝：f32 的内存布局就是设备要的字节布局
            lyre_audio_simd::f32_as_bytes(&self.samples)
        } else {
            lyre_audio_simd::write(&self.samples, format.samples, &mut self.bytes);
            &self.bytes
        };

        if stream
            .render
            .write_to_device(frames, payload, None)
            .is_err()
        {
            log::warn!("独占输出写入设备失败，停流");
            self.stop_stream();
            return;
        }
        self.shared
            .written
            .fetch_add(needed as u64, Ordering::SeqCst);

        self.check_drained();
    }

    /// 推送已结束且环已排空：整首放完了，停流并如实上报。
    fn check_drained(&mut self) {
        if !self.shared.producer_done.load(Ordering::Acquire) {
            return;
        }
        if self.reader.pending() > 0 {
            self.drain_deadline = None;
            return;
        }

        // 环空了不代表设备缓冲里那点内容已经播完。`stop_stream` 会把缓冲一起丢掉，
        // 立刻停会把最后几毫秒真实音频切掉，所以再写一小会儿静音等它走完。
        let deadline = *self
            .drain_deadline
            .get_or_insert_with(|| Instant::now() + DRAIN_LINGER);
        if Instant::now() < deadline {
            return;
        }

        self.shared.finished.store(true, Ordering::SeqCst);
        self.stop_stream();
    }

    fn shutdown(&mut self) {
        self.stop_stream();
        self.stream = None;
    }
}

/// 增益包络。
#[derive(Clone, Copy, Debug)]
struct Fade {
    from: f32,
    factor: f32,
    target: f32,
    then: FadeThen,
    start: Instant,
    duration: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FadeThen {
    Nothing,
    Pause,
    Stop,
}

impl Fade {
    fn idle() -> Self {
        Self {
            from: 1.0,
            factor: 1.0,
            target: 1.0,
            then: FadeThen::Nothing,
            start: Instant::now(),
            duration: Duration::ZERO,
        }
    }

    fn to(&mut self, target: f32, then: FadeThen) {
        self.from = self.factor;
        self.target = target.clamp(0.0, 1.0);
        self.then = then;
        self.start = Instant::now();
        self.duration = FADE;
    }

    /// 立即到位，不淡。
    fn jump(&mut self, value: f32) {
        self.from = value;
        self.factor = value;
        self.target = value;
        self.then = FadeThen::Nothing;
        self.duration = Duration::ZERO;
    }

    /// 推进到当前时刻，返回这一步要执行的动作（只返回一次）。
    fn advance(&mut self) -> FadeThen {
        if self.factor != self.target {
            let elapsed = self.start.elapsed();
            if elapsed < self.duration {
                let progress = elapsed.as_secs_f32() / self.duration.as_secs_f32();
                self.factor = ease(self.from, self.target, progress);
                return FadeThen::Nothing;
            }
            // 走完了：落点严格等于目标，不做浮点外推
            self.factor = self.target;
        }

        // 到位了。挂起的动作只执行一次 —— 纯淡入（then 为 Nothing）也走这里，返回 Nothing
        let then = self.then;
        self.then = FadeThen::Nothing;
        then
    }
}

/// 线性插值到 `progress` 处，落点严格夹在两端之间。
fn ease(from: f32, target: f32, progress: f32) -> f32 {
    from + (target - from) * progress.clamp(0.0, 1.0)
}

/// 欠载统计。独占模式下欠载说明推送跟不上，是需要知道的信号，但不能每轮都打日志。
struct Underrun {
    samples: u64,
    since: Instant,
}

impl Underrun {
    fn new() -> Self {
        Self {
            samples: 0,
            since: Instant::now(),
        }
    }

    fn reset(&mut self) {
        self.samples = 0;
        self.since = Instant::now();
    }

    fn record(&mut self, missing: usize) {
        self.samples += missing as u64;
        if self.since.elapsed() >= UNDERRUN_REPORT {
            log::warn!("独占输出欠载：最近这段时间缺了 {} 个采样", self.samples);
            self.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fade_lands_exactly_on_its_target() {
        let mut fade = Fade::idle();
        fade.to(0.0, FadeThen::Stop);

        // 时长清零即「立刻走完」，不必在用例里真的等 30ms
        fade.duration = Duration::ZERO;
        assert_eq!(fade.advance(), FadeThen::Stop);
        assert_eq!(fade.factor, 0.0, "落点必须严格等于目标");

        // 动作只执行一次
        assert_eq!(fade.advance(), FadeThen::Nothing);
    }

    #[test]
    fn a_fade_in_starts_from_where_the_gain_currently_is() {
        let mut fade = Fade::idle();
        fade.to(0.0, FadeThen::Pause);
        fade.duration = Duration::ZERO;
        fade.advance();

        fade.to(1.0, FadeThen::Nothing);
        assert_eq!(fade.from, 0.0, "应当从当前增益开始爬，而不是跳回 1.0");
        fade.duration = Duration::ZERO;
        fade.advance();
        assert_eq!(fade.factor, 1.0);
    }

    #[test]
    fn a_jump_skips_the_ramp_entirely() {
        let mut fade = Fade::idle();
        fade.to(0.0, FadeThen::Stop);
        fade.jump(1.0);

        assert_eq!(fade.factor, 1.0);
        assert_eq!(fade.advance(), FadeThen::Nothing, "跳变之后不该还挂着动作");
    }

    #[test]
    fn ease_stays_between_the_endpoints() {
        assert_eq!(ease(0.0, 1.0, 0.0), 0.0);
        assert_eq!(ease(0.0, 1.0, 0.5), 0.5);
        assert_eq!(ease(0.0, 1.0, 1.0), 1.0);
        // 进度越界也要夹住
        assert_eq!(ease(0.0, 1.0, 2.0), 1.0);
        assert_eq!(ease(0.0, 1.0, -1.0), 0.0);
        assert_eq!(ease(1.0, 0.0, 0.25), 0.75);
    }

    #[test]
    fn an_idle_fade_never_reports_an_action() {
        let mut fade = Fade::idle();
        for _ in 0..3 {
            assert_eq!(fade.advance(), FadeThen::Nothing);
        }
        assert_eq!(fade.factor, 1.0);
    }
}
