//! 解码层，包括解码器、淡入、下混等。
//! 译码器的输出是 f32，采样率和声道数由解码器决定。解码器输出的采样率和声道数可能不符合播放器要求，
//! 需要在解码器和播放器之间做重采样和下混。

mod decoder;
mod dsp;
mod fade;
mod source;

pub use decoder::SymphoniaDecoder;
pub use fade::FadeIn;
pub use source::LockFreeSource;
