//! 独占输出的渲染线程：命令处理、事件驱动的样本搬运、音量与淡入淡出。
//! COM 对象只在创建它的那条线程上使用，控制线程只经命令通道与它打交道。
//! 淡入淡出按挂钟时间推进 —— 设备停转时按帧计数走不完淡出，Stop/Pause 的状态会卡住。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use lyre_audio_simd::PcmFormat;

use crate::device::{ExclusiveFormat, OpenStream, RenderStream};
use crate::ring::RingReader;

/// 等待设备事件的最长时间。命令也靠这一拍轮询，所以不能取太长。
const EVENT_TIMEOUT_MS: u32 = 5;
/// 不在播放状态时的轮询间隔。
const IDLE_SLEEP: Duration = Duration::from_millis(10);
/// 单轮最多搬运的采样数。约 170ms @48k 立体声，远大于任何设备周期，
/// 一次分配到位之后循环里不再扩容。
const MAX_CHUNK_SAMPLES: usize = 16_384;
/// 淡入淡出时长。够抹平接缝又听不出爬升。
pub const FADE: Duration = Duration::from_millis(30);
/// 欠载日志的最小间隔，避免异常时刷屏。
const UNDERRUN_REPORT: Duration = Duration::from_secs(1);
/// 环排空之后再写这么久静音才收尾。备份缓冲只有一个设备周期（几毫秒），
/// 取一个周期再多一点的余量。
const DRAIN_LINGER: Duration = Duration::from_millis(20);
/// 一首放完之后还留着独占流多久。下一首通常几十毫秒后就到，接在同一条流上即可；
/// 立刻停流的话，下次起播要重新抢占设备、重新锁时钟 —— 独占下那一下听得见。
const IDLE_GRACE: Duration = Duration::from_secs(1);

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
    /// 读取端只在下次读的时候按世代丢掉旧的存量，新写进来的采样照常交付。
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
    /// 已经建立的流中途失效（拔卡、驱动复位、写入失败）。控制线程据此退避重建。
    DeviceLost {
        message: String,
    },
}

/// 渲染线程与控制线程共享的几个标志。
#[derive(Clone)]
pub struct RenderShared {
    /// 推送线程的世代号。每次换手（`rebuild`）加一，初值 1 表示「还没有任何一代」。
    producer_epoch: Arc<AtomicU64>,
    /// 已经写完的那一代。它等于 [`Self::producer_epoch`] 才算「这一代写完了」。
    /// 换手不作废旧标志：上一代晚到的「写完了」记的是旧号，作不了新一代的数。
    producer_done_epoch: Arc<AtomicU64>,
    /// 已写进设备的样本数，控制线程据此算播放位置。
    pub written: Arc<AtomicU64>,
    /// 整首自然放完。
    pub finished: Arc<AtomicBool>,
    /// 累计欠载采样数：环里没货、只能拿静音顶上去的那部分。
    /// 只在推送线程还没报完工时计 —— 曲尾按 `DRAIN_LINGER` 故意补的静音不算。
    pub starved: Arc<AtomicU64>,
    /// 流起播过几次。同一台设备上切歌不该让它涨 —— 涨了就是流被停掉又起了，
    /// 重新抢占设备与锁时钟在独占下听得见。
    pub stream_starts: Arc<AtomicU64>,
}

impl RenderShared {
    pub fn new() -> Self {
        Self {
            producer_epoch: Arc::new(AtomicU64::new(1)),
            producer_done_epoch: Arc::new(AtomicU64::new(0)),
            written: Arc::new(AtomicU64::new(0)),
            finished: Arc::new(AtomicBool::new(false)),
            starved: Arc::new(AtomicU64::new(0)),
            stream_starts: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 起一代新的推送线程，返回它的世代号。调用方要把它交给 [`crate::push::spawn`]。
    pub fn begin_producer(&self) -> u64 {
        self.producer_epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// 交给推送线程的完工标志；它写进去的是自己那一代的世代号。
    pub fn done_flag(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.producer_done_epoch)
    }

    /// 当前这一代推送线程是否已经把整首写完。
    pub fn producer_done(&self) -> bool {
        self.producer_done_epoch.load(Ordering::Acquire)
            == self.producer_epoch.load(Ordering::Acquire)
    }
}

impl Default for RenderShared {
    fn default() -> Self {
        Self::new()
    }
}

/// 渲染线程主体。返回时独占流已经关掉。
/// 两个通道按值收：线程主体要长期持有它们。
#[allow(clippy::needless_pass_by_value)]
pub fn run(
    commands: Receiver<RenderCommand>,
    responses: Sender<RenderResponse>,
    reader: RingReader,
    shared: RenderShared,
    open: OpenStream,
) {
    let mut renderer = Renderer {
        reader,
        stream: None,
        format: None,
        volume: 1.0,
        fade: Fade::idle(),
        playing: false,
        drain_deadline: None,
        release_deadline: None,
        shared,
        samples: Vec::with_capacity(MAX_CHUNK_SAMPLES),
        bytes: Vec::with_capacity(MAX_CHUNK_SAMPLES * 4),
        underrun: Underrun::new(),
        open,
        responses: responses.clone(),
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
                other => renderer.handle(other),
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
        renderer.release_if_idle();
    }

    renderer.shutdown();
    log::info!("独占输出渲染线程已退出");
}

struct Renderer {
    reader: RingReader,
    stream: Option<Box<dyn RenderStream>>,
    format: Option<ExclusiveFormat>,
    volume: f32,
    fade: Fade,
    playing: bool,
    /// 环已排空、正在等设备缓冲播完的时刻。
    drain_deadline: Option<Instant>,
    /// 已上报放完、独占流还留着的截止时刻。到点还没有新曲子就把设备放掉。
    release_deadline: Option<Instant>,
    shared: RenderShared,
    samples: Vec<f32>,
    bytes: Vec<u8>,
    underrun: Underrun,
    open: OpenStream,
    responses: Sender<RenderResponse>,
}

impl Renderer {
    fn handle(&mut self, command: RenderCommand) {
        match command {
            RenderCommand::Shutdown => {}

            RenderCommand::Configure { device, preferred } => {
                // 独占是排他的：不先把手上的流放掉，新的那条永远开不起来 —— 设备已经被自己占住了。
                // 顺序错了表现为「初始化超时」，而不是什么好懂的报错。
                self.stop_stream();
                self.stream = None;
                self.drain_deadline = None;
                self.release_deadline = None;

                let opened = (self.open)(device.as_deref(), preferred);
                let reply = match opened {
                    Ok(stream) => {
                        let format = stream.format();
                        let device_name = stream.device_name().to_owned();
                        let device_endpoint = stream.device_endpoint().to_owned();
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
                    Err(err) => {
                        self.format = None;
                        RenderResponse::Failed(err.to_string())
                    }
                };
                let _ = self.responses.send(reply);
            }

            RenderCommand::Start => self.start(),
            RenderCommand::Pause => {
                self.release_deadline = None;
                self.fade.to(0.0, FadeThen::Pause);
            }
            RenderCommand::Stop => {
                self.release_deadline = None;
                self.fade.to(0.0, FadeThen::Stop);
            }
            RenderCommand::SetVolume(volume) => self.volume = volume.clamp(0.0, 1.0),

            RenderCommand::Flush => {
                self.shared.written.store(0, Ordering::SeqCst);
                self.shared.finished.store(false, Ordering::SeqCst);
                self.fade = Fade::idle();
                self.drain_deadline = None;
                self.release_deadline = None;
                self.underrun.reset();
            }
        }
    }

    fn start(&mut self) {
        // 新曲子来了：宽限作废，这一首接在还开着的那条流上（若上一首刚放完，流还没放掉）
        self.release_deadline = None;
        if self.playing {
            return;
        }
        let Some(stream) = self.stream.as_ref() else {
            return;
        };
        let started = stream.start();
        if let Err(err) = started {
            self.fail_stream(format!("启动独占流失败：{err}"));
            return;
        }
        self.shared.stream_starts.fetch_add(1, Ordering::Relaxed);
        self.playing = true;
        self.drain_deadline = None;
        // 从当前增益淡入到 1.0（暂停后恢复就从 0 爬起来）
        self.fade.to(1.0, FadeThen::Nothing);
    }

    /// 放完之后的空转宽限到了：把独占流关掉，别再占着设备。
    fn release_if_idle(&mut self) {
        let Some(deadline) = self.release_deadline else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }
        self.release_deadline = None;
        self.stop_stream();
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
            let _ = stream.stop();
        }
        self.playing = false;
    }

    /// 设备操作失败：停掉这条流并让控制线程知道。
    /// `playing` 必须同步置假，否则外层循环会立刻再次 `pump`，在死设备上空转。
    fn fail_stream(&mut self, message: String) {
        log::warn!("独占输出中断：{message}");
        self.stop_stream();
        self.stream = None;
        self.format = None;
        self.drain_deadline = None;
        self.release_deadline = None;
        let _ = self.responses.send(RenderResponse::DeviceLost { message });
    }

    /// 等一次设备事件，把缓冲里的空间填满。
    fn pump(&mut self) {
        let Some(format) = self.format else {
            return;
        };
        if self.stream.is_none() {
            return;
        }

        // 先区分「等到超时」和「设备报错」：超时是正常轮询，不能当成流失效
        let waited = self
            .stream
            .as_ref()
            .map(|stream| stream.wait_event(EVENT_TIMEOUT_MS));
        match waited {
            Some(Ok(true)) => {}
            Some(Ok(false)) | None => return,
            Some(Err(err)) => {
                self.fail_stream(format!("等待设备事件失败：{err}"));
                return;
            }
        }

        let available = match self.stream.as_ref().map(|stream| stream.available_frames()) {
            Some(Ok(frames)) => frames,
            Some(Err(err)) => {
                self.fail_stream(format!("查询设备空间失败：{err}"));
                return;
            }
            None => return,
        };

        let channels = usize::from(format.channels.max(1));
        let frames = available.min(MAX_CHUNK_SAMPLES / channels);
        if frames == 0 {
            return;
        }
        let needed = frames * channels;

        self.samples.clear();
        self.samples.resize(needed, 0.0);
        let filled = self.reader.read(&mut self.samples);
        if filled < needed {
            // 环里暂时没货：剩下的保持静音（欠载），绝不能在这里阻塞实时线程
            let missing = needed - filled;
            // 曲尾推送线程已经收工，那段静音是故意补的，不算欠载
            if !self.shared.producer_done() {
                self.shared
                    .starved
                    .fetch_add(missing as u64, Ordering::Relaxed);
            }
            self.underrun.record(missing);
        }

        let gain = self.volume * self.fade.factor;
        if gain != 1.0 {
            for sample in &mut self.samples {
                *sample *= gain;
            }
        }

        let written = {
            let payload: &[u8] = if format.samples == PcmFormat::F32 {
                // 零拷贝：f32 的内存布局就是设备要的字节布局
                lyre_audio_simd::f32_as_bytes(&self.samples)
            } else {
                lyre_audio_simd::write(&self.samples, format.samples, &mut self.bytes);
                &self.bytes
            };
            self.stream
                .as_ref()
                .map(|stream| stream.write(frames, payload))
        };
        match written {
            Some(Ok(())) => {}
            Some(Err(err)) => {
                self.fail_stream(format!("写入设备失败：{err}"));
                return;
            }
            None => return,
        }
        // 只把有效音频算进播放位置：收工后写进去的是撑住设备的静音，
        // 跟着它跑会让拿不到时长的曲目（Matroska 之类）放完后进度条继续爬。
        let progress = if self.shared.producer_done() {
            filled as u64
        } else {
            needed as u64
        };
        self.shared.written.fetch_add(progress, Ordering::SeqCst);

        self.check_drained();
    }

    /// 推送已结束且环已排空：整首放完了，如实上报，但先别停流 ——
    /// 宽限到点还没有新曲子，才交给 [`Self::release_if_idle`] 把设备放掉。
    fn check_drained(&mut self) {
        if !self.shared.producer_done() {
            return;
        }
        if self.reader.pending() > 0 {
            self.drain_deadline = None;
            return;
        }

        // 环空了不代表设备缓冲里那点内容已经播完，再写一小会儿静音等它走完
        let deadline = *self
            .drain_deadline
            .get_or_insert_with(|| Instant::now() + DRAIN_LINGER);
        if Instant::now() < deadline {
            return;
        }

        if !self.shared.finished.load(Ordering::Acquire) {
            self.shared.finished.store(true, Ordering::SeqCst);
            self.release_deadline = Some(Instant::now() + IDLE_GRACE);
        }
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

/// 欠载统计。
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
    use std::sync::Mutex;

    use super::*;
    use crate::device::{OpenStream, RenderStream};
    use crate::test_support::{FakeControl, FakeStream};

    type Controls = Arc<Mutex<Vec<Arc<FakeControl>>>>;

    fn opener(controls: Controls) -> OpenStream {
        Box::new(move |_, _| {
            let control = Arc::new(FakeControl::default());
            controls
                .lock()
                .expect("controls lock")
                .push(Arc::clone(&control));
            Ok(Box::new(FakeStream { control }) as Box<dyn RenderStream>)
        })
    }

    fn renderer(open: OpenStream) -> (Renderer, Receiver<RenderResponse>) {
        let (_ring, _writer, reader) = crate::ring::SampleRing::new();
        let (responses, response_rx) = crossbeam_channel::bounded(8);
        let renderer = Renderer {
            reader,
            stream: None,
            format: None,
            volume: 1.0,
            fade: Fade::idle(),
            playing: false,
            drain_deadline: None,
            release_deadline: None,
            shared: RenderShared::new(),
            samples: Vec::with_capacity(MAX_CHUNK_SAMPLES),
            bytes: Vec::with_capacity(MAX_CHUNK_SAMPLES * 4),
            underrun: Underrun::new(),
            open,
            responses,
        };
        (renderer, response_rx)
    }

    fn configured() -> (Renderer, Receiver<RenderResponse>, Controls) {
        let controls = Arc::new(Mutex::new(Vec::new()));
        let (mut renderer, responses) = renderer(opener(Arc::clone(&controls)));
        renderer.handle(RenderCommand::Configure {
            device: None,
            preferred: None,
        });
        assert!(matches!(
            responses.recv_timeout(Duration::from_secs(1)),
            Ok(RenderResponse::Configured { .. })
        ));
        renderer.handle(RenderCommand::Start);
        assert!(renderer.playing, "Start 之后应当进入播放");
        (renderer, responses, controls)
    }

    fn control(controls: &Controls, index: usize) -> Arc<FakeControl> {
        Arc::clone(&controls.lock().expect("controls lock")[index])
    }

    /// 5ms 等不到设备事件是正常轮询，不能把流打断。
    #[test]
    fn a_wait_timeout_is_not_treated_as_a_device_failure() {
        let (mut renderer, responses, controls) = configured();
        control(&controls, 0).timeout.store(true, Ordering::Relaxed);

        renderer.pump();

        assert!(renderer.playing, "超时只是这一拍没空间，流还好着");
        assert!(renderer.stream.is_some());
        assert!(responses.try_recv().is_err(), "超时不该上报故障");
    }

    #[test]
    fn a_device_event_failure_stops_the_stream_and_reports_it() {
        let (mut renderer, responses, controls) = configured();
        let failed = control(&controls, 0);
        failed.fail_wait.store(true, Ordering::Relaxed);

        renderer.pump();

        assert!(!renderer.playing, "流断了就不能继续假装在播");
        assert!(renderer.stream.is_none());
        match responses.recv_timeout(Duration::from_secs(1)) {
            Ok(RenderResponse::DeviceLost { message }) => {
                assert!(
                    message.contains("等待设备事件失败"),
                    "原因不能丢：{message}"
                );
            }
            other => panic!("应当上报 DeviceLost，实际 {other:?}"),
        }
        assert_eq!(
            failed.wait_calls.load(Ordering::Relaxed),
            1,
            "失败的流不该再被轮询"
        );
    }

    #[test]
    fn a_space_query_failure_stops_the_stream_and_reports_it() {
        let (mut renderer, responses, controls) = configured();
        control(&controls, 0)
            .fail_space
            .store(true, Ordering::Relaxed);

        renderer.pump();

        assert!(!renderer.playing);
        assert!(renderer.stream.is_none());
        match responses.recv_timeout(Duration::from_secs(1)) {
            Ok(RenderResponse::DeviceLost { message }) => {
                assert!(
                    message.contains("查询设备空间失败"),
                    "原因不能丢：{message}"
                );
            }
            other => panic!("应当上报 DeviceLost，实际 {other:?}"),
        }
    }

    #[test]
    fn a_write_failure_stops_the_stream_and_reports_it() {
        let (mut renderer, responses, controls) = configured();
        control(&controls, 0)
            .fail_write
            .store(true, Ordering::Relaxed);

        renderer.pump();

        assert!(!renderer.playing);
        assert!(renderer.stream.is_none());
        match responses.recv_timeout(Duration::from_secs(1)) {
            Ok(RenderResponse::DeviceLost { message }) => {
                assert!(message.contains("写入设备失败"), "原因不能丢：{message}");
            }
            other => panic!("应当上报 DeviceLost，实际 {other:?}"),
        }
    }

    #[test]
    fn a_fresh_stream_resumes_after_a_device_failure() {
        let (mut renderer, responses, controls) = configured();
        control(&controls, 0)
            .fail_write
            .store(true, Ordering::Relaxed);
        renderer.pump();
        assert!(!renderer.playing);
        assert!(matches!(
            responses.recv_timeout(Duration::from_secs(1)),
            Ok(RenderResponse::DeviceLost { .. })
        ));

        // 设备回来了：重新协商并起播，样本要重新往设备里写
        renderer.handle(RenderCommand::Configure {
            device: None,
            preferred: None,
        });
        assert!(matches!(
            responses.recv_timeout(Duration::from_secs(1)),
            Ok(RenderResponse::Configured { .. })
        ));
        renderer.handle(RenderCommand::Start);
        assert!(renderer.playing);

        renderer.pump();
        assert!(
            renderer.shared.written.load(Ordering::SeqCst) > 0,
            "新流应当重新写设备"
        );
    }

    /// 设备已经死掉时，渲染循环必须停在下一次 `wait_event` 上，而不是空转刷 CPU。
    #[test]
    fn the_render_loop_does_not_spin_on_a_dead_device() {
        let controls = Arc::new(Mutex::new(Vec::new()));
        let (commands, command_rx) = crossbeam_channel::bounded(8);
        let (responses, response_rx) = crossbeam_channel::bounded(8);
        let (_ring, _writer, reader) = crate::ring::SampleRing::new();

        let opener_controls = Arc::clone(&controls);
        let open: OpenStream = Box::new(move |_, _| {
            let control = Arc::new(FakeControl::default());
            control.fail_wait.store(true, Ordering::Relaxed);
            control.wait_delay_ms.store(1, Ordering::Relaxed);
            opener_controls
                .lock()
                .expect("controls lock")
                .push(Arc::clone(&control));
            Ok(Box::new(FakeStream { control }) as Box<dyn RenderStream>)
        });

        let render_handle = std::thread::spawn(move || {
            run(command_rx, responses, reader, RenderShared::new(), open);
        });

        commands
            .send(RenderCommand::Configure {
                device: None,
                preferred: None,
            })
            .expect("渲染线程还在");
        assert!(matches!(
            response_rx.recv_timeout(Duration::from_secs(1)),
            Ok(RenderResponse::Configured { .. })
        ));
        commands.send(RenderCommand::Start).expect("渲染线程还在");
        assert!(matches!(
            response_rx.recv_timeout(Duration::from_secs(1)),
            Ok(RenderResponse::DeviceLost { .. })
        ));

        // 设备已经死了，这一段时间里不该再有任何设备操作
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(
            control(&controls, 0).wait_calls.load(Ordering::Relaxed),
            1,
            "死设备被反复轮询就是热循环"
        );

        commands
            .send(RenderCommand::Shutdown)
            .expect("渲染线程还在");
        render_handle.join().expect("渲染线程应当退出");
    }

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

    /// 完工按世代判定：换手之后上一代的「写完了」不替新一代作数，作废时不必清标志。
    #[test]
    fn completion_belongs_to_a_generation_not_to_a_boolean() {
        let shared = RenderShared::new();
        assert!(!shared.producer_done(), "还没起过推送线程，谈不上写完");

        let first = shared.begin_producer();
        assert!(!shared.producer_done(), "刚起的线程还没写完");
        shared.done_flag().store(first, Ordering::Release);
        assert!(shared.producer_done(), "它写完就该报完工");

        // 换手：上一代的完工标志不能替新一代作数
        let second = shared.begin_producer();
        assert!(!shared.producer_done(), "新一代还没写完，不能算完工");

        // 上一代晚到的「写完了」记的是旧号，也不该把新一代顶成完工
        shared.done_flag().store(first, Ordering::Release);
        assert!(!shared.producer_done(), "上一代的完工标志不该替新一代作数");

        shared.done_flag().store(second, Ordering::Release);
        assert!(shared.producer_done());
    }
}
