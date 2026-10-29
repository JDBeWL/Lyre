//! WASAPI 独占（位完美）输出后端：样本直接写进设备的硬件缓冲，中间没有系统混音器重采样。
//! 格式协商优先要声源自己的采样率与声道数，要不到才退回标准档位并对声源重采样。
//! 只在 Windows 上可编译；线程分工：控制线程协商、建链、下发命令，渲染线程持有独占流并搬样本，推送线程按水位灌环。

mod backend;
mod device;
mod error;
mod push;
mod render;
mod resample;
mod ring;

pub use backend::ExclusiveBackend;
pub use device::ExclusiveFormat;
pub use error::{ExclusiveError, Result};
pub use resample::RubatoResampleSource;
pub use ring::{RING_SAMPLES, RingReader, RingWriter, SampleRing};

#[cfg(test)]
mod hardware {
    use std::time::Duration;

    use lyre_audio::AudioBackend;

    use crate::backend::ExclusiveBackend;

    /// 枚举默认输出设备上的全部音频会话。
    /// COM 必须先在调用线程上初始化成 MTA，否则每个调用都会失败；重复初始化是安全的。
    fn audio_sessions(label: &str) -> Vec<(u32, String)> {
        use wasapi::{DeviceEnumerator, Direction, initialize_mta};

        let _ = initialize_mta();
        let enumerator = DeviceEnumerator::new().expect("创建设备枚举器失败");
        let device = enumerator
            .get_default_device(&Direction::Render)
            .expect("取默认输出设备失败");
        let manager = device.get_iaudiosessionmanager().expect("取会话管理器失败");
        let list = manager
            .get_audiosessionenumerator()
            .expect("取会话枚举器失败");
        let count = list.get_count().expect("取会话数失败");

        let mut found = Vec::new();
        for index in 0..count {
            let Ok(session) = list.get_session(index) else {
                continue;
            };
            let (Ok(pid), Ok(state)) = (session.get_process_id(), session.get_state()) else {
                continue;
            };
            println!("{label} 会话 #{index}：pid={pid} state={state}");
            found.push((pid, state.to_string()));
        }
        found
    }

    /// 当前进程的音频会话状态；音量合成器就是按这个列表列出应用的。
    fn our_sessions(label: &str) -> Vec<String> {
        let ours = std::process::id();
        audio_sessions(label)
            .into_iter()
            .filter(|(pid, _)| *pid == ours)
            .map(|(_, state)| state)
            .collect()
    }

    /// 写一个全零的 16-bit PCM WAV —— 用例只验通路，不想真的发出声音。
    fn silent_wav(dir: &tempfile::TempDir, sample_rate: u32, channels: u16) -> std::path::PathBuf {
        silent_wav_frames(dir, sample_rate, channels, 4 * 8_000)
    }

    /// 同上，但指定帧数。
    fn silent_wav_frames(
        dir: &tempfile::TempDir,
        sample_rate: u32,
        channels: u16,
        frames: usize,
    ) -> std::path::PathBuf {
        let frames: usize = frames;
        let bits: u16 = 16;
        let block_align = channels * bits / 8;
        let data_len = (frames * channels as usize * 2) as u32;

        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(44 + data_len as usize, 0);

        let path = dir
            .path()
            .join(format!("silence-{frames}-{sample_rate}.wav"));
        std::fs::write(&path, bytes).expect("写入测试 WAV 失败");
        path
    }

    /// 真机会走完「协商独占格式 → 建链 → 推送 → 独占写设备」整条路。
    ///
    /// 默认忽略：它会真的占住默认输出设备。需要时单独跑：
    /// `cargo test -p lyre-audio-exclusive -- --ignored --nocapture`
    #[test]
    #[ignore = "需要真实声卡，会独占默认输出设备"]
    fn the_exclusive_stream_opens_and_plays_a_silent_track() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = silent_wav(&dir, 44_100, 2);

        let mut backend = ExclusiveBackend::open(None).expect("默认设备应当能开独占流");
        let detail = backend.describe_output();
        println!("协商到的独占格式：{detail}");
        assert!(!detail.is_empty(), "应当报出协商到的口径");

        let duration = backend.load(&path, true).expect("应当能装载");
        assert!(duration > Duration::ZERO, "时长不该为零");

        // 素材是 44.1k 立体声：协商按优先级要声源自己的口径，要到了就是位完美直出
        let format = backend.format().expect("装载之后应当有格式");
        println!("装载后的独占格式：{}", backend.describe_output());
        assert!(
            format.is_bit_perfect_for(44_100, 2),
            "44.1k 素材应当协商到 44.1k 直出，实际 {}",
            backend.describe_output()
        );

        // 位置必须真的往前走：只有渲染线程在往设备写，它才会动
        let mut advanced = false;
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(20));
            if backend.position() > Duration::ZERO {
                advanced = true;
                break;
            }
        }
        assert!(advanced, "独占输出没有推进播放位置");

        std::thread::sleep(Duration::from_millis(200));
        backend.stop();
        drop(backend);
    }

    /// 独占的核心特征是这台设备被别人拿不走：独占期间用 cpal 再要一条共享流去碰同一台设备，要不到才说明声音绕开了系统混音器。
    /// 共享流打不开 ⟹ 音量合成器里看见的进程条目是系统保留的历史会话，不是我们的流。
    #[test]
    #[ignore = "需要真实声卡，会独占默认输出设备"]
    fn an_exclusive_stream_keeps_shared_streams_out() {
        use rodio::cpal::traits::{DeviceTrait, HostTrait};

        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = silent_wav(&dir, 44_100, 2);

        let mut backend = ExclusiveBackend::open(None).expect("默认设备应当能开独占流");
        backend.load(&path, true).expect("应当能装载");
        std::thread::sleep(Duration::from_millis(300));

        let host = rodio::cpal::default_host();
        let device = host.default_output_device().expect("应当有默认输出设备");
        let config = device.default_output_config().expect("应当有默认配置");

        let opened = match config.sample_format() {
            rodio::cpal::SampleFormat::F32 => device.build_output_stream(
                config.into(),
                |data: &mut [f32], _: &_| data.fill(0.0),
                |err| eprintln!("共享流错误：{err}"),
                None,
            ),
            other => panic!("默认配置不是 f32（{other:?}），这条用例只处理 f32"),
        };

        println!(
            "独占期间再开共享流的结果：{}",
            match &opened {
                Ok(_) => "成功（不该发生）".to_owned(),
                Err(err) => format!("被拒绝：{err}"),
            }
        );
        assert!(
            opened.is_err(),
            "独占生效时共享流不该还能打开设备，实际却成功了 —— 说明这条流并没有真正独占"
        );

        drop(backend);
    }

    /// 反直觉的事实：**WASAPI 独占流自身也会注册音频会话**，所以音量合成器必然列出这个应用 —— 与独占有没有生效是两回事。
    /// 独占的证据在别处：这条流开着时，别的共享流打不开这台设备（见 [`an_exclusive_stream_keeps_shared_streams_out`]）。
    #[test]
    #[ignore = "需要真实声卡，会独占默认输出设备"]
    fn exclusive_playback_registers_a_session_of_its_own() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = silent_wav(&dir, 44_100, 2);
        let mut backend = ExclusiveBackend::open(None).expect("默认设备应当能开独占流");
        backend.load(&path, true).expect("应当能装载");
        std::thread::sleep(Duration::from_millis(400));

        // 不能断言「开流之前一个都没有」：同一进程里先跑过别的真机用例的话，那些流留下的
        // 会话在进程退出前一直是 Inactive 的，这条对照就不成立了。看的是它确实注册了一个活的。
        let during = our_sessions("独占中");
        println!("本进程：独占中 {during:?}");
        assert!(
            during.iter().any(|state| state == "Active"),
            "预期独占流会注册一个处于 Active 的会话。这里只有 {during:?}，说明系统行为变了"
        );

        drop(backend);
    }

    /// 挑一台现在能开独占的设备：先是系统默认，被别的程序占着就往下找。
    /// 默认设备被占用是常态 —— 本应用自己开着的时候就是。
    fn open_any_exclusive() -> ExclusiveBackend {
        let mut candidates: Vec<Option<String>> = vec![None];
        candidates.extend(
            lyre_audio::device::enumerate_output_devices()
                .into_iter()
                .map(|device| Some(device.id)),
        );

        let mut last = String::new();
        for id in candidates {
            match ExclusiveBackend::open(id.as_deref()) {
                Ok(backend) => return backend,
                Err(err) => last = err.to_string(),
            }
        }
        panic!("没有一台输出设备能开独占：{last}");
    }

    /// 等流起播到第 `want` 次。`Start` 是异步下发的，装载返回时渲染线程可能还没处理到。
    fn wait_starts(backend: &ExclusiveBackend, want: u64) {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_millis(800) {
            if backend.stream_starts() >= want {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "等了 800ms 流起播次数还是 {}，到不了 {want}",
            backend.stream_starts()
        );
    }

    /// 等这一首自然放完。
    fn wait_finished(backend: &mut ExclusiveBackend, limit: Duration) {
        let start = std::time::Instant::now();
        while start.elapsed() < limit {
            if backend.is_finished() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("等了 {limit:?} 这一首还没放完");
    }

    /// 独占下切歌不该重开流。
    ///
    /// 一首放完就把流停掉的话，下一首起播要重新抢占设备、重新锁时钟 —— 实测那正是接缝处
    /// 听得见的那一下（seek 一路不停流，所以它从来不卡）。放完之后留一段宽限，下一首直接
    /// 接在同一条还开着的流上；宽限过了还没新曲子，设备才让出去。
    #[test]
    #[ignore = "需要真实声卡，会独占默认输出设备"]
    fn a_track_change_reuses_the_running_stream() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let first = silent_wav(&dir, 48_000, 2);
        let second = silent_wav(&dir, 48_000, 2);
        let third = silent_wav(&dir, 48_000, 2);

        let mut backend = open_any_exclusive();
        backend.load(&first, true).expect("应当能装载");
        wait_starts(&backend, 1);

        wait_finished(&mut backend, Duration::from_secs(5));
        // 真实链路里下一首是几十毫秒之后到的（音频线程一拍 + 界面一帧）
        backend.load(&second, true).expect("应当能装载");
        // 留出渲染线程处理这条 Start 的时间：真重开了流，这一等就会把次数顶上去
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            backend.stream_starts(),
            1,
            "紧接着切歌不该重开独占流：停掉再起会有一下听得见的卡顿"
        );
        // 换手那一瞬可能空一拍设备周期（推送线程刚起、渲染线程正好在读），
        // 那只有几毫秒；上百毫秒量级的断供才说明推送跟不上。
        assert!(
            backend.underrun_samples() < 2_048,
            "换手让环断供了 {} 个采样",
            backend.underrun_samples()
        );

        std::thread::sleep(Duration::from_millis(200));
        assert!(backend.position() > Duration::ZERO, "第二首应当在往前走");

        // 等第二首也放完，再过掉宽限期：这时候设备早该让出去了
        wait_finished(&mut backend, Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1_400));

        backend.load(&third, true).expect("应当能装载");
        wait_starts(&backend, 2);

        backend.stop();
        drop(backend);
    }

    /// 跳转之后必须马上接上新位置的声音，且不能把「播到哪了」和「整首放完」搞错。
    /// 独占下的 seek 要重建整条链 —— 链得在旧的那条还在播的时候先备好，换手才只剩作废环那一瞬；
    /// 先停旧线程再去打开解码器、seek 的话，这段耗时就是实打实的断音。
    #[test]
    #[ignore = "需要真实声卡，会独占默认输出设备"]
    fn seeking_resumes_from_the_target() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = silent_wav(&dir, 44_100, 2);

        let mut backend = ExclusiveBackend::open(None).expect("默认设备应当能开独占流");
        let duration = backend.load(&path, true).expect("应当能装载");
        std::thread::sleep(Duration::from_millis(120));

        let before = backend.position();
        assert!(before > Duration::ZERO, "应当已经播了一点，实际 {before:?}");

        let target = duration / 2;
        backend.seek(target).expect("跳转应当成功");

        let after = backend.position();
        println!("跳到 {target:?}：跳完之后报 {after:?}");
        assert!(
            after >= target.saturating_sub(Duration::from_millis(50)),
            "跳完之后位置不该退回去：目标是 {target:?}，实际 {after:?}"
        );
        assert!(!backend.is_finished(), "跳转不该把这一首判成放完");

        std::thread::sleep(Duration::from_millis(200));
        let later = backend.position();
        println!("再等 200ms 后 {later:?}");
        assert!(
            later > after,
            "跳完之后声音应当接着走：{after:?} → {later:?}"
        );
        assert!(
            later <= duration,
            "位置不该越过总时长：{later:?} > {duration:?}"
        );

        backend.stop();
        drop(backend);
    }

    /// 关掉独占流后它留下的会话会怎样 —— 这决定「不播放时把流放掉」能不能让音量合成器里的条目一并收掉。
    #[test]
    #[ignore = "需要真实声卡，会独占默认输出设备"]
    fn what_happens_to_the_session_after_the_exclusive_stream_closes() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = silent_wav(&dir, 44_100, 2);

        let mut backend = ExclusiveBackend::open(None).expect("默认设备应当能开独占流");
        backend.load(&path, true).expect("应当能装载");
        std::thread::sleep(Duration::from_millis(400));
        println!("播放中：{:?}", our_sessions("播放中"));

        backend.pause();
        std::thread::sleep(Duration::from_millis(200));
        println!("暂停后：{:?}", our_sessions("暂停后"));

        drop(backend);
        println!("刚关流：{:?}", our_sessions("刚关流"));

        std::thread::sleep(Duration::from_millis(1500));
        println!("关流 1.5s 后：{:?}", our_sessions("关流后"));
    }

    /// 切换设备时进度必须接着走。独占下的 `set_device` 要重建整条链路，拿链路起点
    /// （`current.base`）重建就会把上次手动跳转之后播过的那一段全丢掉，进度条当场退回去。
    #[test]
    #[ignore = "需要真实声卡，会独占默认输出设备"]
    fn switching_device_keeps_the_playback_position() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = silent_wav(&dir, 44_100, 2);

        let mut backend = ExclusiveBackend::open(None).expect("默认设备应当能开独占流");
        backend.load(&path, true).expect("应当能装载");
        std::thread::sleep(Duration::from_millis(300));

        let before = backend.position();
        assert!(
            before > Duration::from_millis(100),
            "应当已经播了一段，实际 {before:?}"
        );

        // 换一台不存在的设备：会退回默认设备、重建整条链路，进度必须接着走
        backend
            .set_device(Some("wasapi:{0.0.0.00000000}.{no-such-endpoint}"))
            .expect("切设备应当成功");

        let after = backend.position();
        println!("切换前 {before:?} → 切换后 {after:?}");
        assert!(
            after >= before.saturating_sub(Duration::from_millis(150)),
            "换流之后进度退回去了：{before:?} → {after:?}"
        );

        std::thread::sleep(Duration::from_millis(200));
        let later = backend.position();
        println!("再等 200ms 后 {later:?}");
        assert!(
            later > after,
            "换流之后进度应当继续推进：{after:?} → {later:?}"
        );

        drop(backend);
    }
}

/// 注入式测试设施：假设备流 + 不碰声卡的 WAV 素材。真机用例仍走 `hardware`。
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::time::Duration;

    use lyre_audio_simd::PcmFormat;

    use crate::device::{ExclusiveFormat, RenderStream};
    use crate::error::{ExclusiveError, Result};

    /// 假流统一报的口径，与 [`silent_wav`] 的素材一致，免去重采样。
    pub const FORMAT: ExclusiveFormat = ExclusiveFormat {
        sample_rate: 44_100,
        channels: 2,
        samples: PcmFormat::F32,
        block_align: 8,
    };

    /// 用例摆布假流行为的旋钮。流被渲染线程持有后，测试仍通过它注入故障。
    pub struct FakeControl {
        pub wait_calls: AtomicUsize,
        pub write_calls: AtomicUsize,
        pub fail_wait: AtomicBool,
        pub fail_space: AtomicBool,
        pub fail_write: AtomicBool,
        /// 让下一次等待按「超时」返回，用来验证超时不会被当成设备故障。
        pub timeout: AtomicBool,
        /// 每次等事件的挂钟耗时；不为零时渲染线程不会把整首瞬间写完。
        pub wait_delay_ms: AtomicU64,
        pub available_frames: AtomicUsize,
    }

    impl Default for FakeControl {
        fn default() -> Self {
            Self {
                wait_calls: AtomicUsize::new(0),
                write_calls: AtomicUsize::new(0),
                fail_wait: AtomicBool::new(false),
                fail_space: AtomicBool::new(false),
                fail_write: AtomicBool::new(false),
                timeout: AtomicBool::new(false),
                wait_delay_ms: AtomicU64::new(5),
                available_frames: AtomicUsize::new(480),
            }
        }
    }

    pub struct FakeStream {
        pub control: Arc<FakeControl>,
    }

    impl RenderStream for FakeStream {
        fn format(&self) -> ExclusiveFormat {
            FORMAT
        }

        fn device_name(&self) -> &'static str {
            "Fake Device"
        }

        fn device_endpoint(&self) -> &'static str {
            "fake-endpoint"
        }

        fn start(&self) -> Result<()> {
            Ok(())
        }

        fn stop(&self) -> Result<()> {
            Ok(())
        }

        fn wait_event(&self, _timeout_ms: u32) -> Result<bool> {
            self.control.wait_calls.fetch_add(1, Ordering::Relaxed);
            let delay = self.control.wait_delay_ms.load(Ordering::Relaxed);
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
            if self.control.timeout.load(Ordering::Relaxed) {
                return Ok(false);
            }
            if self.control.fail_wait.load(Ordering::Relaxed) {
                return Err(ExclusiveError::Write("fake wait failure".to_owned()));
            }
            Ok(true)
        }

        fn available_frames(&self) -> Result<usize> {
            if self.control.fail_space.load(Ordering::Relaxed) {
                return Err(ExclusiveError::Write("fake space failure".to_owned()));
            }
            Ok(self.control.available_frames.load(Ordering::Relaxed))
        }

        fn write(&self, _frames: usize, _payload: &[u8]) -> Result<()> {
            self.control.write_calls.fetch_add(1, Ordering::Relaxed);
            if self.control.fail_write.load(Ordering::Relaxed) {
                return Err(ExclusiveError::Write("fake write failure".to_owned()));
            }
            Ok(())
        }
    }

    /// 44.1k / 2ch / 16-bit 全零 WAV。只用来喂真实解码器。
    pub fn silent_wav(dir: &Path, frames: usize) -> PathBuf {
        let sample_rate: u32 = 44_100;
        let channels: u16 = 2;
        let bits: u16 = 16;
        let block_align = channels * bits / 8;
        let data_len = (frames * channels as usize * 2) as u32;

        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(44 + data_len as usize, 0);

        let path = dir.join(format!("fake-silence-{frames}.wav"));
        std::fs::write(&path, bytes).expect("写入测试 WAV 失败");
        path
    }
}
