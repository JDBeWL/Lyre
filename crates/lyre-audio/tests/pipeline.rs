//! 音频链路集成测试：`AudioHandle::spawn_with` 注入假后端，不需要声卡。
//! 完整验证「命令 → 线程 → 事件 / 快照」，包括只在真机偶发的时序问题（切歌瞬间误报结束）。
//!
//! 测试里 unwrap/expect 是正当的断言手段。`clippy.toml` 的 `allow-*-in-tests`
//! 只覆盖 `#[cfg(test)]` 模块，不覆盖 `tests/` 下的独立集成测试 crate，故在此显式允许。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use lyre_audio::{
    AudioBackend, AudioError, AudioHandle, PlaybackState, PlayerCommand, PlayerEvent, Result,
};

#[derive(Default)]
struct FakeState {
    finished: AtomicBool,
    position_ms: AtomicU64,
    volume_bits: AtomicU32,
}

/// 不碰声卡的后端。行为完全由测试摆布。
struct FakeBackend {
    state: Arc<FakeState>,
}

impl FakeBackend {
    fn new(state: Arc<FakeState>) -> Self {
        Self { state }
    }
}

impl AudioBackend for FakeBackend {
    fn load(&mut self, path: &Path, _autoplay: bool) -> Result<Duration> {
        if path.to_string_lossy().contains("broken") {
            return Err(AudioError::Decode("fake decode failure".to_owned()));
        }
        self.state.position_ms.store(0, Relaxed);
        self.state.finished.store(false, Relaxed);
        Ok(Duration::from_secs(180))
    }

    fn play(&mut self) -> Result<()> {
        self.state.finished.store(false, Relaxed);
        Ok(())
    }

    fn pause(&mut self) {}

    fn stop(&mut self) {
        self.state.finished.store(true, Relaxed);
    }

    fn seek(&mut self, position: Duration) -> Result<()> {
        self.state
            .position_ms
            .store(position.as_millis() as u64, Relaxed);
        Ok(())
    }

    fn set_volume(&mut self, volume: f32) {
        self.state.volume_bits.store(volume.to_bits(), Relaxed);
    }

    fn position(&self) -> Duration {
        Duration::from_millis(self.state.position_ms.load(Relaxed))
    }

    fn is_finished(&self) -> bool {
        self.state.finished.load(Relaxed)
    }
}

/// 启一个连着假后端的音频线程，返回句柄、事件接收端和可摆布的后端状态。
fn harness() -> (AudioHandle, Receiver<PlayerEvent>, Arc<FakeState>) {
    let (tx, rx) = channel();
    let state = Arc::new(FakeState::default());
    let moved = Arc::clone(&state);

    let handle = AudioHandle::spawn_with(tx, move || {
        Ok(Box::new(FakeBackend::new(moved)) as Box<dyn AudioBackend>)
    })
    .expect("测试里起音频线程不该失败");

    (handle, rx, state)
}

fn wait_for<F>(rx: &Receiver<PlayerEvent>, mut predicate: F) -> Option<PlayerEvent>
where
    F: FnMut(&PlayerEvent) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(event) if predicate(&event) => return Some(event),
            // 超时与「拿到但不匹配」都继续等；`continue` 在这里是多余的
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
    None
}

#[test]
fn load_reports_duration_and_starts_playing() {
    let (handle, rx, _state) = harness();

    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/RABBIT.flac"),
        autoplay: true,
    });

    let event =
        wait_for(&rx, |e| matches!(e, PlayerEvent::Loaded { .. })).expect("应当收到 Loaded");
    match event {
        PlayerEvent::Loaded { path, duration } => {
            assert!(path.ends_with("RABBIT.flac"));
            assert_eq!(duration, Duration::from_secs(180));
        }
        _ => unreachable!(),
    }

    assert_eq!(
        handle.duration(),
        Duration::from_secs(180),
        "快照应当同步时长"
    );
    assert_eq!(handle.snapshot().state(), PlaybackState::Playing);
}

#[test]
fn load_without_autoplay_stays_paused() {
    let (handle, rx, _state) = harness();

    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/夢我夢中.flac"),
        autoplay: false,
    });

    wait_for(&rx, |e| matches!(e, PlayerEvent::State(_))).expect("应当收到状态变化");
    assert_eq!(handle.snapshot().state(), PlaybackState::Paused);
}

#[test]
fn toggle_play_flips_between_paused_and_playing() {
    let (handle, rx, _state) = harness();

    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/夢我夢中.flac"),
        autoplay: false,
    });
    wait_for(&rx, |e| matches!(e, PlayerEvent::State(_)));

    handle.send(PlayerCommand::TogglePlay);
    wait_for(&rx, |e| {
        matches!(e, PlayerEvent::State(PlaybackState::Playing))
    });
    assert_eq!(handle.snapshot().state(), PlaybackState::Playing);

    handle.send(PlayerCommand::TogglePlay);
    wait_for(&rx, |e| {
        matches!(e, PlayerEvent::State(PlaybackState::Paused))
    });
    assert_eq!(handle.snapshot().state(), PlaybackState::Paused);
}

#[test]
fn seek_is_reflected_in_snapshot_immediately() {
    let (handle, rx, _state) = harness();

    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/夢我夢中.flac"),
        autoplay: true,
    });
    wait_for(&rx, |e| matches!(e, PlayerEvent::Loaded { .. }));

    handle.send(PlayerCommand::Seek(Duration::from_secs(75)));

    // 快照走的是无锁原子量，不等事件、不等下一次 tick 就该跟手
    let deadline = Instant::now() + Duration::from_secs(2);
    while handle.position() != Duration::from_secs(75) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(handle.position(), Duration::from_secs(75));
}

#[test]
fn volume_updates_event_and_snapshot_and_clamps() {
    let (handle, rx, state) = harness();

    handle.send(PlayerCommand::SetVolume(0.35));
    let event = wait_for(&rx, |e| matches!(e, PlayerEvent::Volume(_))).expect("应当收到音量事件");
    assert_eq!(event, PlayerEvent::Volume(0.35));
    assert!((handle.volume() - 0.35).abs() < f32::EPSILON);

    handle.send(PlayerCommand::SetVolume(4.0));
    let deadline = Instant::now() + Duration::from_secs(2);
    while handle.volume() != 1.0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(handle.volume(), 1.0, "越界音量应当被夹住");
    assert_eq!(
        f32::from_bits(state.volume_bits.load(Relaxed)),
        1.0,
        "后端收到的也应当是夹过的值"
    );
}

#[test]
fn natural_end_is_reported_exactly_once() {
    let (handle, rx, state) = harness();

    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/夢我夢中.flac"),
        autoplay: true,
    });
    wait_for(&rx, |e| matches!(e, PlayerEvent::Loaded { .. }));

    // 先播一会儿：线程要看到过一次「未播空」，之后 empty() 的结论才被采信（防切歌瞬间误报）
    std::thread::sleep(Duration::from_millis(150));

    state.finished.store(true, Relaxed);
    let first = wait_for(&rx, |e| matches!(e, PlayerEvent::Ended));
    assert!(first.is_some(), "自然播完应当上报 Ended");

    // 再等几拍，确认不会重复上报
    let mut extra = 0;
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline {
        if let Ok(PlayerEvent::Ended) = rx.recv_timeout(Duration::from_millis(50)) {
            extra += 1;
        }
    }
    assert_eq!(extra, 0, "Ended 不允许重复上报，否则会自动连跳好几首");
}

#[test]
fn freshly_loaded_track_does_not_falsely_report_end() {
    let (handle, rx, state) = harness();
    // 模拟「刚 append、音频还没被消费」的窗口期：empty() 暂时为 true
    state.finished.store(true, Relaxed);

    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/夢我夢中.flac"),
        autoplay: true,
    });
    wait_for(&rx, |e| matches!(e, PlayerEvent::Loaded { .. }));

    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Ok(PlayerEvent::Ended) = rx.recv_timeout(Duration::from_millis(50)) {
            panic!("切歌瞬间不应误报播放结束");
        }
    }
}

#[test]
fn decode_failure_surfaces_as_an_error_not_a_panic() {
    let (handle, rx, _state) = harness();

    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/broken.flac"),
        autoplay: true,
    });

    let event = wait_for(&rx, |e| matches!(e, PlayerEvent::Error(_))).expect("坏文件应当上报错误");
    match event {
        PlayerEvent::Error(message) => assert!(message.contains("fake decode failure")),
        _ => unreachable!(),
    }
    assert_eq!(handle.snapshot().state(), PlaybackState::Idle);
}

#[test]
fn undecodable_device_does_not_prevent_commands_from_returning() {
    let (tx, rx) = channel();
    let handle = AudioHandle::spawn_with(tx, || {
        Err(AudioError::DeviceOpen("no audio device".to_owned()))
    })
    .expect("起线程本身应当成功，打不开的是设备");

    handle.send(PlayerCommand::Play);

    let event =
        wait_for(&rx, |e| matches!(e, PlayerEvent::Error(_))).expect("设备打不开应当上报错误");
    match event {
        PlayerEvent::Error(message) => {
            assert_eq!(message, "无法打开音频输出设备：no audio device");
        }
        _ => unreachable!(),
    }

    // 关键：设备不可用时后续命令不能 panic，程序要能继续跑
    handle.send(PlayerCommand::SetVolume(0.5));
    handle.send(PlayerCommand::Seek(Duration::from_secs(10)));
    handle.send(PlayerCommand::Pause);
}

#[test]
fn commands_after_a_failed_device_open_keep_reporting() {
    // 第一次失败时 `open` 已经被 take 走，后续命令走的是「不再重试」那条路。
    // 那一条也必须报出来，否则上层 `load` 乐观设成的播放状态永远没人纠正。
    let (tx, rx) = channel();
    let handle = AudioHandle::spawn_with(tx, || {
        Err(AudioError::DeviceOpen("no audio device".to_owned()))
    })
    .expect("起线程本身应当成功，打不开的是设备");

    handle.send(PlayerCommand::Play);
    wait_for(&rx, |e| matches!(e, PlayerEvent::Error(_))).expect("第一次打不开要报错");

    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/夢我夢中.flac"),
        autoplay: true,
    });
    wait_for(&rx, |e| matches!(e, PlayerEvent::Error(_))).expect("设备已经废了，Load 也必须报出来");
}

#[test]
fn enumerate_devices_reports_a_list() {
    let (handle, rx, _state) = harness();

    handle.send(PlayerCommand::EnumerateDevices);

    let event = wait_for(&rx, |e| matches!(e, PlayerEvent::Devices(_))).expect("应当收到设备列表");
    match event {
        // 没声卡的机器上拿到空列表也是正常结果，这里只要求列表本身可用
        PlayerEvent::Devices(devices) => {
            assert!(devices.iter().all(|device| !device.id.is_empty()));
        }
        _ => unreachable!(),
    }
}

#[test]
fn selecting_a_device_gets_an_acknowledgement() {
    let (handle, rx, _state) = harness();

    handle.send(PlayerCommand::SetDevice { id: None });

    let event = wait_for(&rx, |e| matches!(e, PlayerEvent::DeviceChanged { .. }))
        .expect("切换设备应当有回执");
    // `None` 表示跟随系统默认，回执里的 name 是那一刻默认设备的友好名
    assert!(matches!(event, PlayerEvent::DeviceChanged { id: None, .. }));
}

#[test]
fn switching_device_mid_playback_does_not_report_the_track_as_ended() {
    let (handle, rx, state) = harness();

    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/夢我夢中.flac"),
        autoplay: true,
    });
    wait_for(&rx, |e| matches!(e, PlayerEvent::Loaded { .. }));
    // 先让它播一会儿，`seen_content` 才会置真 —— 之后 empty() 的结论才被采信
    std::thread::sleep(Duration::from_millis(150));

    // 换流的那一瞬间新音源还没出声，empty() 会短暂为 true
    state.finished.store(true, Relaxed);
    handle.send(PlayerCommand::SetDevice { id: None });

    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Ok(PlayerEvent::Ended) = rx.recv_timeout(Duration::from_millis(50)) {
            panic!("换设备不该被当成播放结束，否则界面会当场跳下一曲");
        }
    }
}

#[test]
fn dropping_the_handle_shuts_the_thread_down() {
    let (handle, rx, _state) = harness();
    handle.send(PlayerCommand::Load {
        path: PathBuf::from("/music/夢我夢中.flac"),
        autoplay: true,
    });
    wait_for(&rx, |e| matches!(e, PlayerEvent::Loaded { .. }));

    drop(handle);

    // 句柄 drop 后线程退出，事件发送端随之关闭
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut disconnected = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Err(RecvTimeoutError::Disconnected) => {
                disconnected = true;
                break;
            }
            // 超时与「拿到事件」都继续等：`continue` 是多余的（匹配已是循环体最后一句）
            Err(RecvTimeoutError::Timeout) | Ok(_) => {}
        }
    }
    assert!(disconnected, "句柄析构之后音频线程应当收工");
}
