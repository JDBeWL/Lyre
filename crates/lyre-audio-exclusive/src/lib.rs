//! WASAPI 独占（位完美）输出后端。
//!
//! 与共享模式的区别只有一处：共享模式下音频要经过系统混音器，采样率与格式都会被它改一遍；
//! 独占模式是应用直接把样本写进设备的硬件缓冲，中间没有任何重采样。
//!
//! 因此格式协商的策略是**优先要声源自己的采样率与声道数**：要到了就是位完美直出，
//! 要不到（设备只认固定口径）才退回标准档位并对声源重采样。
//!
//! 线程分工：
//! - 控制线程：实现 `AudioBackend`，负责协商、建链、下发命令；
//! - 渲染线程：独占流的所有权在这里，事件驱动地把环里的样本搬进设备，并负责音量与淡入淡出；
//! - 推送线程：把解码链路的输出灌进环，靠水位门控不把内存撑爆。
//!
//! 本 crate 用的是 `wasapi` 的安全封装，全程零 `unsafe`：自带的 `[lints]` 里
//! `unsafe_code` 仍是 `forbid`，真正放开 `unsafe` 的只有 `lyre-audio-simd`。
//!
//! 只在 Windows 上可编译 —— `wasapi` 是 windows-only 依赖，而这条产线本身也只跑 Windows。

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

    /// 枚举默认输出设备上的全部音频会话，返回（进程号, 状态）。
    ///
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

    /// 当前进程的音频会话状态，按枚举顺序。音量合成器就是按这个列表列出应用的。
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
        const FRAMES: usize = 4 * 8_000;
        let bits: u16 = 16;
        let block_align = channels * bits / 8;
        let data_len = (FRAMES * channels as usize * 2) as u32;

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

        let path = dir.path().join("silence.wav");
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

        // 素材是 44.1k 立体声。协商优先要声源自己的口径，要到了就是位完美直出 ——
        // 这条断言同时也是「协商策略真的按优先级走」的现场证据。
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

        // 再走一会儿，确认没有欠载与卡死
        std::thread::sleep(Duration::from_millis(200));
        backend.stop();
        drop(backend);
    }

    /// 独占的核心特征就是「这台设备被别人拿不走」。用 cpal 再要一条共享流去碰同一台设备 ——
    /// 要不到才说明声音确实绕开了系统混音器；要得到就说明名为独占、实为共享。
    ///
    /// 这条用例专门用来回答「为什么系统音量合成器里还看得见这个进程」：
    /// 共享流打不开 ⟹ 条目是系统保留的历史会话，不是我们的流。
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
                &config.into(),
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

    /// 记录一个反直觉的事实：**WASAPI 独占流自身也会注册音频会话**，所以系统音量合成器里
    /// 必然会列出这个应用 —— 这跟「独占有没有生效」是两回事。
    ///
    /// 独占的证据在别处：这条流开着时，别的共享流打不开这台设备
    /// （见 [`an_exclusive_stream_keeps_shared_streams_out`]）。
    ///
    /// 留着这条用例是为了钉住这个认知：一旦系统行为变了，这里会先红，音量合成器相关的结论才需要重看。
    #[test]
    #[ignore = "需要真实声卡，会独占默认输出设备"]
    fn exclusive_playback_registers_a_session_of_its_own() {
        let before = our_sessions("开始前");
        assert!(
            before.is_empty(),
            "开流之前本进程不该有会话，否则这个对照就失去意义了：{before:?}"
        );

        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let path = silent_wav(&dir, 44_100, 2);
        let mut backend = ExclusiveBackend::open(None).expect("默认设备应当能开独占流");
        backend.load(&path, true).expect("应当能装载");
        std::thread::sleep(Duration::from_millis(400));

        let during = our_sessions("独占中");
        println!("本进程：独占中 {during:?}");
        assert!(
            !during.is_empty(),
            "预期独占流会注册自己的会话。这里开始为空，说明系统行为变了"
        );

        drop(backend);
    }

    /// 关掉独占流之后它留下的会话会怎样 —— 这决定了「不播放时把流放掉」能不能让
    /// 音量合成器里的条目一并收掉。只记录事实，不预设结论。
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

        // 换成一台不存在的设备：会退回默认设备、重建整条链路。进度必须接着走，不能退回开头。
        backend
            .set_device(Some("wasapi:{0.0.0.00000000}.{no-such-endpoint}"))
            .expect("切设备应当成功");

        let after = backend.position();
        println!("切换前 {before:?} → 切换后 {after:?}");
        assert!(
            after >= before.saturating_sub(Duration::from_millis(150)),
            "换流之后进度退回去了：{before:?} → {after:?}"
        );

        // 新链路还得真的接着往下走
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
