//! Lyre 的领域内核：曲目模型、播放队列语义、目录扫描。
//! 不依赖 GPUI 也不依赖声卡，可脱离窗口与音频设备单测。

pub mod collections;
pub mod error;
pub mod group;
pub mod lyrics;
pub mod metadata;
pub mod playlist;
pub mod scanner;
pub mod track;

pub use collections::{Collection, CollectionId, Collections};
pub use error::{CoreError, Result};
pub use group::{
    ARTIST_SEPARATORS, AlbumGroup, ArtistGroup, UNKNOWN_ALBUM, UNKNOWN_ARTIST, VARIOUS_ARTISTS,
    album_key, album_key_str, artist_key_str, artist_keys, artist_keys_str, group_by_album,
    group_by_artist,
};
pub use lyrics::{
    LyricLine, LyricWord, Lyrics, load_for_audio as load_lyrics_for_audio, sidecar_path,
};
pub use metadata::{Cover, read_cover, read_track};
pub use playlist::{Advance, PlayMode, Playlist, SortKey};
pub use scanner::{
    AUDIO_EXTENSIONS, SIGNATURE_PROBE_LEN, Signature, UNSUPPORTED_EXTENSIONS, collect_audio_paths,
    collect_audio_paths_with, is_audio_extension, is_audio_file, is_decodable_extension,
    scan_into_tracks, signature_of,
};
pub use track::{AudioInfo, Quality, Track, TrackId, format_duration};
