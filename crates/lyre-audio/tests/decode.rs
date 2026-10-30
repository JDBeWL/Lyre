//! 解码链路集成测试：格式探测 → 解码 → 独立线程 → 有界通道 → EOF 判定，重点是结束判定。
//! `allow-*-in-tests` 只覆盖 `#[cfg(test)]` 模块，`tests/` 下要在这里显式允许 unwrap/expect。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::time::Duration;

use lyre_audio::{AudioError, LockFreeSource, SymphoniaDecoder, TrackDecoder, WhenEmpty};
use rodio::Source as _;

/// 手写一个 16-bit PCM 的 WAV。
fn write_wav(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    frames: usize,
    sample_at: impl Fn(usize, usize) -> f32,
) {
    const BITS: u16 = 16;
    let block_align = channels * BITS / 8;
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = (frames * channels as usize * 2) as u32;

    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");

    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&BITS.to_le_bytes());

    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());

    for frame in 0..frames {
        for channel in 0..channels as usize {
            let value = sample_at(frame, channel).clamp(-1.0, 1.0);
            let quantized = (value * 32_767.0).round() as i16;
            bytes.extend_from_slice(&quantized.to_le_bytes());
        }
    }

    std::fs::write(path, bytes).expect("写入测试 WAV 失败");
}

fn temp_wav(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("创建临时目录失败");
    let path = dir.path().join(name);
    (dir, path)
}

/// 独占推送线程那条路（`WhenEmpty::Wait`）不许交出补出来的静音：素材是正弦，
/// 连续 ≥64 个恰好 0.0 只可能来自补白。
#[test]
fn the_head_of_a_track_never_gets_padded_with_invented_silence() {
    use std::num::NonZeroU16;
    use std::sync::Arc;

    use lyre_audio::decode::{
        ChannelConvertSource, GainControl, GainRamp, LfeConfig, SoftClip, WhenEmpty,
    };

    const RATE: u32 = 48_000;
    let (_dir, path) = temp_wav("head.wav");
    write_wav(&path, RATE, 2, RATE as usize * 4, |frame, channel| {
        let frequency = if channel == 0 { 440.0 } else { 660.0 };
        let seconds = frame as f32 / RATE as f32;
        0.5 * (std::f32::consts::TAU * frequency * seconds).sin()
    });

    let decoder = TrackDecoder::open(&path).expect("打开");
    let source = LockFreeSource::new(decoder, WhenEmpty::Wait).expect("建链");
    let converted = ChannelConvertSource::new(
        source,
        NonZeroU16::new(2).expect("2 声道"),
        LfeConfig::default(),
    );
    let control = Arc::new(GainControl::new());
    let ramped = GainRamp::new(
        converted,
        Duration::from_millis(60),
        RATE,
        2,
        Arc::clone(&control),
    );
    let mut chain = SoftClip::new(ramped);

    // 照推送线程那样一口气拉 40 万个样本（约 4 秒 @48k 立体声）
    let head: Vec<f32> = chain.by_ref().take(400_000).collect();
    assert!(head.len() > 300_000, "拉到的样本太少：{}", head.len());

    let longest = head
        .split(|sample| *sample != 0.0)
        .map(<[f32]>::len)
        .max()
        .unwrap_or(0);
    assert!(
        longest < 64,
        "头一批样本里有 {longest} 个连着的 0.0 —— 那是通道空着时补出来的静音，不是素材"
    );
}

#[test]
fn reports_format_and_duration_from_the_container() {
    let (_dir, path) = temp_wav("tone.wav");
    write_wav(&path, 44_100, 2, 4410, |_, _| 0.5);

    let decoder = SymphoniaDecoder::new(&path).expect("应当能打开 WAV");
    assert_eq!(decoder.sample_rate(), 44_100);
    assert_eq!(decoder.channels(), 2);
    assert_eq!(decoder.source_channels(), 2);

    let duration = decoder.total_duration().expect("WAV 应当能报出时长");
    assert!(
        (duration.as_secs_f64() - 0.1).abs() < 0.001,
        "时长应为 100ms，实际 {duration:?}"
    );
}

#[test]
fn decodes_pcm_to_interleaved_stereo_f32() {
    let (_dir, path) = temp_wav("tone.wav");
    write_wav(&path, 44_100, 2, 4410, |_, _| 0.5);

    let decoder = SymphoniaDecoder::new(&path).unwrap();
    let samples: Vec<f32> = decoder.collect();

    assert_eq!(samples.len(), 4410 * 2, "两个声道都要解出来");
    // 16-bit 量化误差在 1/32767 量级
    assert!(
        samples.iter().all(|s| (s - 0.5).abs() < 1e-3),
        "应当是一个 0.5 的直流信号"
    );
}

#[test]
fn mono_survives_the_decoder_and_is_promoted_later_on_the_chain() {
    let (_dir, path) = temp_wav("mono.wav");
    write_wav(&path, 8_000, 1, 100, |_, _| 0.25);

    let decoder = SymphoniaDecoder::new(&path).unwrap();
    assert_eq!(decoder.source_channels(), 1, "源文件是单声道");
    assert_eq!(
        decoder.channels(),
        1,
        "声道数由输出设备决定，解码器不再替它做主"
    );

    let samples: Vec<f32> = decoder.collect();
    assert_eq!(samples.len(), 100, "100 帧单声道 = 100 个样本");
    assert!(
        samples.iter().all(|s| (s - 0.25).abs() < 1e-3),
        "解码阶段不该改动样本值"
    );
}

#[test]
fn surround_keeps_its_layout_all_the_way_through_the_decoder() {
    let (_dir, path) = temp_wav("surround.wav");
    // FL FR FC LFE SL SR
    write_wav(&path, 8_000, 6, 100, |_, channel| match channel {
        0 => 0.5,
        1 => 0.25, // FR
        3 => 0.4,  // LFE
        _ => 0.0,
    });

    let decoder = SymphoniaDecoder::new(&path).unwrap();
    assert_eq!(decoder.source_channels(), 6);
    assert_eq!(
        decoder.channels(),
        6,
        "下混发生在播放链路上，解码器原样输出源布局"
    );

    let samples: Vec<f32> = decoder.take(12).collect();
    assert_eq!(samples.len(), 12, "两帧 6 声道");

    // LFE 必须原样留着：2.1 折入与立体声下混都靠它，解码阶段丢掉就再也找不回来
    assert!(
        (samples[3] - 0.4).abs() < 0.01,
        "LFE = {}，期望约 0.4",
        samples[3]
    );
    assert!(
        (samples[0] - 0.5).abs() < 0.01,
        "FL = {}，期望约 0.5",
        samples[0]
    );
}

/// 从 ramp 波形（样本值 = 帧号 / 总帧数）反推帧号。
fn frame_of(sample: f32, frames: usize) -> i64 {
    (f64::from(sample) * frames as f64).round() as i64
}

/// symphonia 对 WAV 的 `SeekMode::Accurate` 不做到帧精确，落点恒为数据包边界（本素材 1152 帧/包），
/// 容差取一个包；这个偏差在 44.1kHz 素材上约 26ms。
const SEEK_TOLERANCE_FRAMES: i64 = 1_200;

#[test]
fn seek_lands_within_one_packet_of_the_target() {
    const FRAMES: usize = 80_000;

    let (_dir, path) = temp_wav("ramp.wav");
    write_wav(&path, 8_000, 2, FRAMES, |frame, _| {
        frame as f32 / FRAMES as f32
    });

    let mut decoder = SymphoniaDecoder::new(&path).unwrap();
    decoder.prefill().expect("预填充应当成功");
    decoder
        .seek(Duration::from_millis(500))
        .expect("跳转到 500ms 应当成功");

    let samples: Vec<f32> = decoder.take(16).collect();
    let actual = frame_of(samples[0], FRAMES);
    let expected = 4_000; // 500ms @ 8kHz

    assert!(
        (actual - expected).abs() <= SEEK_TOLERANCE_FRAMES,
        "跳转落点第 {actual} 帧，目标第 {expected} 帧，偏差超出容差"
    );
    assert!(
        actual < expected + 64,
        "跳转不应越过目标位置（落点 {actual}，目标 {expected}）"
    );
}

#[test]
fn seek_before_first_decode_is_deferred_not_an_error() {
    const FRAMES: usize = 80_000;

    let (_dir, path) = temp_wav("ramp.wav");
    write_wav(&path, 8_000, 2, FRAMES, |frame, _| {
        frame as f32 / FRAMES as f32
    });

    // 解码器还没初始化就 seek，切歌后立刻点进度条就是这个顺序，不该报错
    let mut decoder = SymphoniaDecoder::new(&path).unwrap();
    decoder
        .seek(Duration::from_millis(500))
        .expect("初始化前 seek 应当被延后而不是失败");

    let samples: Vec<f32> = decoder.take(16).collect();
    let actual = frame_of(samples[0], FRAMES);
    assert!(
        (actual - 4_000).abs() <= SEEK_TOLERANCE_FRAMES,
        "延后执行的跳转也应当生效，落点第 {actual} 帧"
    );
}

#[test]
fn seeking_past_the_end_does_not_panic() {
    let (_dir, path) = temp_wav("ramp.wav");
    write_wav(&path, 8_000, 2, 800, |_, _| 0.1);

    let mut decoder = SymphoniaDecoder::new(&path).unwrap();
    let _ = decoder.prefill();
    // 目标超出文件长度：允许报错，但绝不能 panic
    let _ = decoder.seek(Duration::from_secs(60));
    let _ = decoder.next();
}

#[test]
fn short_file_still_prefills_successfully() {
    let (_dir, path) = temp_wav("short.wav");
    // 0.1 秒的素材远小于 500ms 的缓冲容量：填不满是正常的，不该被判成坏文件
    write_wav(&path, 8_000, 2, 800, |_, _| 0.5);

    let mut decoder = SymphoniaDecoder::new(&path).unwrap();
    decoder
        .prefill()
        .expect("短文件到达 EOF 就算预填充完成，不应报错");

    let samples: Vec<f32> = decoder.take(2_000).collect();
    assert_eq!(samples.len(), 1_600, "短文件应当被完整解出来");
}

#[test]
fn missing_file_reports_an_error_instead_of_panicking() {
    let Err(error) = SymphoniaDecoder::new(Path::new("/definitely/not/here.wav")) else {
        panic!("不存在的文件居然打开成功了");
    };
    assert!(
        error.to_string().contains("无法打开文件"),
        "实际错误：{error}"
    );
}

#[test]
fn unsupported_content_reports_an_error() {
    let (_dir, path) = temp_wav("not-audio.wav");
    std::fs::write(&path, b"this is definitely not a wav file, it is just text").unwrap();
    assert!(
        SymphoniaDecoder::new(&path).is_err(),
        "非音频内容应当被格式探测拒绝"
    );
}

/// 畸形头：采样率写成 0。时长会算成 `frames / 0.0` = inf，`Duration::from_secs_f64` 对 inf 直接
/// panic —— 它跑在解码线程上，panic 会带走整个进程。拒开或退成可用采样率都算对。
#[test]
fn a_zero_sample_rate_header_never_panics() {
    let (_dir, path) = temp_wav("zero-rate.wav");
    write_wav(&path, 0, 2, 800, |_, _| 0.5);

    if let Ok(decoder) = SymphoniaDecoder::new(&path) {
        assert_ne!(
            decoder.sample_rate(),
            0,
            "0 速率会让缓冲尺寸归零，解出数据也没地方放"
        );
        if let Some(duration) = decoder.total_duration() {
            assert!(
                duration.as_secs() < 3_600,
                "时长必须是个有限值，实际 {duration:?}"
            );
        }
    }
}

/// 每个真实样本至少且恰好交付一次，且最终能判定结束。
/// 断言非零样本数而非总数 —— 欠载时源会吐 `0.0` 静音撑流，无节流的测试线程必然掺进静音。
#[test]
fn lockfree_source_delivers_every_sample_then_reports_end() {
    let (_dir, path) = temp_wav("tone.wav");
    write_wav(&path, 8_000, 2, 800, |_, _| 0.5);

    let decoder = TrackDecoder::open(&path).unwrap();
    let mut source = LockFreeSource::new(decoder, WhenEmpty::Wait).unwrap();

    let mut real = 0usize;
    let mut iterations = 0usize;
    for sample in source.by_ref() {
        iterations += 1;
        assert!(
            iterations <= 2_000_000,
            "解码源迟迟不结束，说明结束判定失效了"
        );
        if sample != 0.0 {
            real += 1;
        }
    }

    assert_eq!(
        real, 1_600,
        "800 帧立体声 = 1600 个真实样本，每个恰好交付一次"
    );
    assert!(source.producer_finished(), "取完之后解码线程应当已经收工");
}

#[test]
fn lockfree_source_forwards_track_metadata() {
    let (_dir, path) = temp_wav("tone.wav");
    write_wav(&path, 22_050, 2, 2_205, |_, _| 0.0);

    let decoder = TrackDecoder::open(&path).unwrap();
    let source = LockFreeSource::new(decoder, WhenEmpty::Wait).unwrap();

    assert_eq!(source.channels().get(), 2);
    assert_eq!(source.sample_rate().get(), 22_050);
    // 恒为 None 是刻意的：见 source.rs 里对「必须自己重采样」的说明
    assert_eq!(source.current_span_len(), None);

    let duration = source.total_duration().expect("应当能报出时长");
    assert!((duration.as_secs_f64() - 0.1).abs() < 0.001);
}

/// 丢掉 `LockFreeSource` 之后，解码线程必须自己收工 —— `Drop` 里已经没有 `join` 兜底了。
/// 素材要远大于有界通道（30 块 × 16384 样本 ≈ 49 万样本）且一帧都不消费，才真的停在阻塞 `send` 上。
/// 判据是文件句柄：线程收工后 `File` 才 drop，Windows 上被占用的文件 `remove_file` 会失败。
#[test]
fn dropping_the_source_releases_the_producer() {
    let (_dir, path) = temp_wav("oversized.wav");
    write_wav(&path, 8_000, 1, 30 * 16_384 * 3, |frame, _| {
        (frame % 200) as f32 / 200.0
    });

    let decoder = TrackDecoder::open(&path).unwrap();
    let source = LockFreeSource::new(decoder, WhenEmpty::Wait).unwrap();
    // 一帧都不读，这 50ms 是给通道填满用的
    std::thread::sleep(Duration::from_millis(50));
    drop(source);

    let mut released = false;
    for _ in 0..300 {
        if std::fs::remove_file(&path).is_ok() {
            released = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        released,
        "drop 之后解码线程应当立刻收工并释放文件句柄 —— 丢掉 receiver 是唯一能解开阻塞 send 的动作"
    );
}

/// 扩展名不可信：APE 改名成 `.wav` 会被 symphonia 的 MPEG 探测当成 MP3 帧同步，解出满刻度噪声。
/// 判据是错误由 `ApeSource` 报出（`Decode`），不是 `Probe`。
#[test]
fn a_renamed_ape_goes_to_the_ape_decoder_instead_of_the_mpeg_probe() {
    let (_dir, path) = temp_wav("disguised.wav");
    let mut bytes = b"MAC ".to_vec();
    bytes.extend(std::iter::repeat_n(0x5a_u8, 64 * 1024));
    std::fs::write(&path, &bytes).unwrap();

    let Err(error) = TrackDecoder::open(&path) else {
        panic!("合成的字节不是合法 APE，不该打开成功");
    };
    assert!(
        matches!(error, AudioError::Decode(_)),
        "应当由 APE 解码器报解码失败，实际：{error}"
    );
}

/// WavPack：symphonia 没有对应的格式读取器，打开只会说「认不出来」。
/// 现在它按 `wvpk` 路由到 `oxideav-wavpack`，坏内容由那个解码器报 `Decode`。
#[test]
fn a_renamed_wavpack_goes_to_the_wavpack_decoder() {
    let (_dir, path) = temp_wav("disguised.flac");
    let mut bytes = b"wvpk".to_vec();
    bytes.extend(std::iter::repeat_n(0x5a_u8, 64 * 1024));
    std::fs::write(&path, &bytes).unwrap();

    let Err(error) = TrackDecoder::open(&path) else {
        panic!("合成的字节不是合法 WavPack，不该打开成功");
    };
    assert!(
        matches!(error, AudioError::Decode(_)),
        "应当由 WavPack 解码器报解码失败，实际：{error}"
    );
}

/// Ogg 容器 symphonia 认得，但里面装 Opus 它没有解码器，只会以 `unsupported audio codec` 失败。
/// 现在按「`OggS` + 偏移 28 处的 `OpusHead`」路由到 `opus-pure`。
#[test]
fn a_renamed_ogg_opus_goes_to_the_opus_decoder() {
    let (_dir, path) = temp_wav("disguised.flac");
    let mut bytes = b"OggS".to_vec();
    bytes.resize(28, 0);
    bytes.extend_from_slice(b"OpusHead");
    bytes.extend(std::iter::repeat_n(0x5a_u8, 4096));
    std::fs::write(&path, &bytes).unwrap();

    let Err(error) = TrackDecoder::open(&path) else {
        panic!("合成的字节不是合法 Ogg 页，不该打开成功");
    };
    assert!(
        matches!(error, AudioError::Decode(_)),
        "应当由 Opus 解码器报解码失败，实际：{error}"
    );
}

/// 通用引擎自己也要挡一道。绕过 `TrackDecoder` 直接建它时，认得出的魔数绝不能落到探测上 ——
/// 那正是「APE 被当成 MP3、解出满刻度噪声」的入口。
#[test]
fn the_generic_engine_refuses_a_magic_it_recognises() {
    let (_dir, path) = temp_wav("disguised.wav");
    let mut bytes = b"MAC ".to_vec();
    bytes.extend(std::iter::repeat_n(0x5a_u8, 4096));
    std::fs::write(&path, &bytes).unwrap();

    let Err(error) = SymphoniaDecoder::new(&path) else {
        panic!("魔数认得出的文件不该落在通用引擎上");
    };
    assert!(
        matches!(&error, AudioError::Unsupported(name) if name == "APE"),
        "要指名是哪种格式，实际：{error}"
    );
}

/// 容器能打开、但一个样本都解不出来 —— 必须报错，不能当成「放完了」：
/// 这首一上来就「放完」，列表循环时看着像无声快进。
#[test]
fn a_container_without_any_audio_data_reports_an_error() {
    let (_dir, path) = temp_wav("empty.wav");
    write_wav(&path, 44_100, 2, 0, |_, _| 0.0);

    let decoder = TrackDecoder::open(&path).unwrap();
    let Err(error) = LockFreeSource::new(decoder, WhenEmpty::Wait) else {
        panic!("一个样本都没有的文件不该建得起解码链路");
    };
    assert!(
        matches!(error, AudioError::NoAudioData),
        "应当是「没有可解码的音频数据」，实际：{error}"
    );
}
