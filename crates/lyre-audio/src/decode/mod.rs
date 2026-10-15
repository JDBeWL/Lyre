//! 解码层：解码器、声道转换、软削波、增益包络、无锁线程隔离。
//! 输出是 f32，采样率与声道数由声源决定；重采样与淡入淡出都在解码器和播放器之间的适配链上完成。

mod ape;
mod clip;
mod convert;
mod decoder;
mod dsp;
mod fade;
mod opus;
mod source;
mod track;
mod wavpack;

pub use clip::SoftClip;
pub use convert::ChannelConvertSource;
pub use decoder::SymphoniaDecoder;
pub use dsp::LfeConfig;
pub use fade::{GainControl, GainRamp};
pub use source::{LockFreeSource, WhenEmpty};
pub use track::TrackDecoder;
