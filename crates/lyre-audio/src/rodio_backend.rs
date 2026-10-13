//! 基于 rodio 的播放后端。
//! rodio 只负责把采样送到声卡，解码全部在 `crate::decode`。
//! 本类型 `!Send`（`MixerDeviceSink` 底层是 cpal 的 `Stream`），只在音频线程内创建和使用。

use std::path::{Path, PathBuf};
use std::time::Duration;

use rodio::source::UniformSourceIterator;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};

use crate::backend::AudioBackend;
use crate::decode::{FadeIn, LockFreeSource, SymphoniaDecoder};
use crate::error::{AudioError, Result};

/// 淡入时长：重建式 seek 从任意波形位置起播，不在零点就是爆音；60ms 够抹平又听不出爬升。
const FADE_IN: Duration = Duration::from_millis(60);

/// 一次 seek 重建的产物。
type BuiltSource = (Box<dyn rodio::Source<Item = f32> + Send>, Duration);

pub struct RodioBackend {
    /// 必须一直持有。一旦 drop，音频流关闭，声音立刻消失。
    device: MixerDeviceSink,
    player: Player,
    current: Option<Loaded>,
    volume: f32,
    /// 播放器是否已「停」（`stop()` 之后、尚未重新装载）。`stop()` 只立标志，真正丢掉音源
    /// 要等下一次 `periodic_access`（约 5ms），那几毫秒里 `empty()` 仍是 `false`。
    stopped: bool,
}

struct Loaded {
    path: PathBuf,
    duration: Duration,
    /// 当前音源的起点。rodio 的 `get_pos()` 在每次 `append` 后从 0 重新计数，
    /// 重建式 seek 之后要把它加回去，否则进度条每次都跳回开头。
    base: Duration,
    /// 重建时需要保持原来的播放/暂停状态
    playing: bool,
}

impl RodioBackend {
    pub fn open() -> Result<Self> {
        let device = DeviceSinkBuilder::open_default_sink()
            .map_err(|err| AudioError::DeviceOpen(err.to_string()))?;

        let player = Player::connect_new(device.mixer());
        // 新建的 Player 默认就是播放中，先压回暂停，避免 load 之后立刻出声
        player.pause();

        Ok(Self {
            device,
            player,
            current: None,
            volume: 1.0,
            stopped: false,
        })
    }

    /// 组装一条完整的解码链, symphonia 解码 → 无锁线程隔离 → 重采样到设备 → 淡入。
    fn build_source(&self, path: &Path, start: Duration) -> Result<BuiltSource> {
        let mut decoder = SymphoniaDecoder::new(path)?;
        let duration = decoder.total_duration().unwrap_or(Duration::ZERO);

        if !start.is_zero() {
            decoder.seek(start)?;
        }

        let channels = self.device.config().channel_count();
        let sample_rate = self.device.config().sample_rate();

        let source = LockFreeSource::new(decoder)?;

        // rodio 内部重采样依赖 `source.current_span_len()`，而 `LockFreeSource`
        // 恒返回 `None`，高采样率音轨会以降速播放
        let resampled = UniformSourceIterator::new(source, channels, sample_rate);

        // 淡入按输出采样率算，必须放在重采样之后
        let faded = FadeIn::new(resampled, FADE_IN, sample_rate.get(), channels.get());

        Ok((Box::new(faded), duration))
    }

    /// 清空队列并把新音源接上，同时恢复之前的音量。
    fn swap_in(&self, source: Box<dyn rodio::Source<Item = f32> + Send>, autoplay: bool) {
        self.player.clear();
        self.player.set_volume(self.volume);
        self.player.append(source);
        if autoplay {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    /// 按当前曲目重建一条音源链路并接上（不改变播放 / 暂停状态）；没装载过时是无操作。
    fn rebuild_current(&mut self) -> Result<()> {
        let Some(current) = self.current.as_ref() else {
            return Ok(());
        };
        let path = current.path.clone();
        let start = current.base;

        let (source, _) = self.build_source(&path, start)?;
        self.player.clear();
        self.player.set_volume(self.volume);
        self.player.append(source);
        Ok(())
    }
}

impl AudioBackend for RodioBackend {
    fn load(&mut self, path: &Path, autoplay: bool) -> Result<Duration> {
        let (source, duration) = self.build_source(path, Duration::ZERO)?;
        self.swap_in(source, autoplay);

        self.current = Some(Loaded {
            path: path.to_path_buf(),
            duration,
            base: Duration::ZERO,
            playing: autoplay,
        });
        self.stopped = false;

        Ok(duration)
    }

    /// 队列空了（`stop()` 之后、或一首自然播完）时 `player.play()` 只放下暂停标志，
    /// 没有音源可播必须按当前曲目重建链路，否则是「无声的正在播放」。
    fn play(&mut self) -> Result<()> {
        if self.stopped || self.player.empty() {
            self.rebuild_current()?;
        }

        self.player.play();
        self.stopped = false;
        if let Some(current) = self.current.as_mut() {
            current.playing = true;
        }
        Ok(())
    }

    fn pause(&mut self) {
        self.player.pause();
        if let Some(current) = self.current.as_mut() {
            current.playing = false;
        }
    }

    fn stop(&mut self) {
        self.player.stop();
        // 保留 current：stop 之后按「上一曲」重播当前曲还要用；起点归零
        if let Some(current) = self.current.as_mut() {
            current.base = Duration::ZERO;
            current.playing = false;
        }
        self.stopped = true;
    }

    /// 重建式 seek，不用 `player.try_seek()`,由于有界通道里已缓存几秒采样，原地跳转既复杂又不精确；
    /// 重解一条链路的代价只是一次几十毫秒的预填充。
    fn seek(&mut self, position: Duration) -> Result<()> {
        let Some(current) = self.current.as_ref() else {
            return Ok(()); // 还没装载任何东西，seek 无意义
        };

        let path = current.path.clone();
        let duration = current.duration;
        let was_playing = current.playing;
        let target = if duration > Duration::ZERO {
            position.min(duration)
        } else {
            position
        };

        let (source, _) = self.build_source(&path, target)?;
        self.swap_in(source, was_playing);

        self.current = Some(Loaded {
            path,
            duration,
            base: target,
            playing: was_playing,
        });
        self.stopped = false;

        Ok(())
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.player.set_volume(self.volume);
    }

    fn position(&self) -> Duration {
        let Some(current) = self.current.as_ref() else {
            return Duration::ZERO;
        };
        let position = current.base + self.player.get_pos();
        if current.duration > Duration::ZERO {
            position.min(current.duration)
        } else {
            position
        }
    }

    fn is_finished(&self) -> bool {
        self.player.empty()
    }
}
