//! 解码链路集成测试
//! 用例自己按 WAV 格式拼字节，不引第三方编码库，免得编码器参与进被测路径。
//! 覆盖格式探测 → 解码 → 下混 → 独立线程 → 有界通道 → EOF 判定，重点是结束判定。
//!
//! 测试里 unwrap/expect 是正当的断言手段。`clippy.toml` 的 `allow-*-in-tests`
//! 只覆盖 `#[cfg(test)]` 模块，不覆盖 `tests/` 下的独立集成测试 crate，故在此显式允许。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::time::Duration;

use lyre_audio::{LockFreeSource, SymphoniaDecoder};
use rodio::Source as _;

/// 手写一个 16-bit PCM 的 WAV
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
fn mono_is_promoted_to_stereo_by_duplication() {
    let (_dir, path) = temp_wav("mono.wav");
    write_wav(&path, 8_000, 1, 100, |_, _| 0.25);

    let decoder = SymphoniaDecoder::new(&path).unwrap();
    assert_eq!(decoder.source_channels(), 1, "源文件是单声道");
    assert_eq!(decoder.channels(), 2, "输出统一为立体声");

    let samples: Vec<f32> = decoder.collect();
    assert_eq!(samples.len(), 200, "100 帧单声道 → 200 个交错样本");
    assert!(
        samples.iter().all(|s| (s - 0.25).abs() < 1e-3),
        "单声道复制到左右两路，值应当一致"
    );
}

#[test]
fn surround_wav_is_downmixed_to_stereo_with_itu_coefficients() {
    let (_dir, path) = temp_wav("surround.wav");
    // FL FR FC LFE SL SR
    write_wav(&path, 8_000, 6, 100, |_, channel| match channel {
        0 => 0.5,
        1 => 0.25, // FR
        _ => 0.0,
    });

    let decoder = SymphoniaDecoder::new(&path).unwrap();
    assert_eq!(decoder.source_channels(), 6);
    assert_eq!(decoder.channels(), 2);

    let samples: Vec<f32> = decoder.take(200).collect();
    assert_eq!(samples.len(), 200, "6 声道应当被压成 2 声道");

    // 只剩 FL/FR，中置与环绕都是 0，归一化系数 0.707
    let expected_left = 0.5 * 0.707;
    let expected_right = 0.25 * 0.707;
    assert!(
        (samples[0] - expected_left).abs() < 0.01,
        "左声道 = {}，期望约 {expected_left}",
        samples[0]
    );
    assert!(
        (samples[1] - expected_right).abs() < 0.01,
        "右声道 = {}，期望约 {expected_right}",
        samples[1]
    );
}

/// 从 ramp 波形（样本值 = 帧号 / 总帧数）反推帧号。
fn frame_of(sample: f32, frames: usize) -> i64 {
    (f64::from(sample) * frames as f64).round() as i64
}

/// symphonia 对 WAV 的 `SeekMode::Accurate` 不做到帧精确，落点恒为数据包边界（本素材 1152 帧/包），
/// 所以断言「落在一个包之内」；这个偏差在 44.1kHz 素材上约 26ms。
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
    // 短文件的预填充会因为提前到达 EOF 而返回失败，这是正常的，不是错误路径
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

/// 畸形头：采样率写成 0。这里要的只是「不 panic」：拒开、或退成可用采样率都算通过。
/// （采样率当有效值用时时长会算成 `frames / 0.0` = inf，而 `Duration::from_secs_f64` 对 inf
/// 直接 panic —— 它跑在解码线程上，panic 会带走整个进程。）
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
    // 格式探测直接拒绝也是正确答案，所以这里不断言 `is_ok`
}

/// 关键用例：每个真实样本至少且恰好交付一次，且最终能判定结束。
/// 断言非零样本数而非总数 —— 欠载时源会吐 `0.0` 静音撑流（设计使然），无节流的测试线程必然掺进静音。
#[test]
fn lockfree_source_delivers_every_sample_then_reports_end() {
    let (_dir, path) = temp_wav("tone.wav");
    write_wav(&path, 8_000, 2, 800, |_, _| 0.5);

    let decoder = SymphoniaDecoder::new(&path).unwrap();
    let mut source = LockFreeSource::new(decoder).unwrap();

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

    let decoder = SymphoniaDecoder::new(&path).unwrap();
    let source = LockFreeSource::new(decoder).unwrap();

    assert_eq!(source.channels().get(), 2);
    assert_eq!(source.sample_rate().get(), 22_050);
    // 恒为 None 是刻意的：见 source.rs 里对「必须自己重采样」的说明
    assert_eq!(source.current_span_len(), None);

    let duration = source.total_duration().expect("应当能报出时长");
    assert!((duration.as_secs_f64() - 0.1).abs() < 0.001);
}

/// 丢掉 `LockFreeSource` 之后，解码线程必须自己收工 —— `Drop` 里已经没有 `join` 兜底了。
/// 造一个远大于有界通道（30 块 × 16384 样本 ≈ 49 万样本）的素材并一个样本都不消费，
/// 让它填满通道后停在阻塞 `send` 上；然后丢掉 source。
///
/// 判据用它持有的文件句柄：线程收工后 `File` 随之 drop，Windows 上「文件仍被占用」会让
/// `remove_file` 失败。其他平台删除打开中的文件本就允许，断言在那里退化为恒真。
#[test]
fn dropping_the_source_releases_the_producer() {
    let (_dir, path) = temp_wav("oversized.wav");
    write_wav(&path, 8_000, 1, 30 * 16_384 * 3, |frame, _| {
        (frame % 200) as f32 / 200.0
    });

    let decoder = SymphoniaDecoder::new(&path).unwrap();
    let source = LockFreeSource::new(decoder).unwrap();
    // 一帧都不读，并且给它一点时间把通道填满 —— 这样才真的走到「阻塞在 send」那条路径
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
