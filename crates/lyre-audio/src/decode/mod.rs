//! 解码层，包括解码器、声道转换、软削波、增益包络、无锁线程隔离。
//! 译码器的输出是 f32，采样率和声道数由声源决定；重采样、布局转换与淡入淡出
//! 都在解码器和播放器之间的适配链上完成。

mod clip;
mod convert;
mod decoder;
mod dsp;
mod fade;
mod source;

pub use clip::SoftClip;
pub use convert::ChannelConvertSource;
pub use decoder::SymphoniaDecoder;
pub use dsp::LfeConfig;
pub use fade::{GainControl, GainRamp};
pub use source::LockFreeSource;
