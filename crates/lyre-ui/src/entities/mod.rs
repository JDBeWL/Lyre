//! 实体层：应用状态的宿主。

pub mod filter;
mod library;
mod playback;

pub use library::{
    CollectionSummary, FAVORITES_NAME, LibraryEntity, LibraryEvent, RepeatState, ScanProgress,
    Scope, ScopeKind, Step, searching,
};
pub use lyre_audio::PlaybackState;
pub use playback::{PlaybackEntity, PlaybackEvent};
