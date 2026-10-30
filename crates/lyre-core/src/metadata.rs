use std::path::Path;
use std::time::Duration;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use lofty::probe::Probe;

use crate::error::{CoreError, Result};
use crate::track::{AudioInfo, Track, TrackId};

/// 内嵌封面的原始字节。解码成位图是渲染层的事，内核只负责把字节拿出来。
#[derive(Clone, Debug)]
pub struct Cover {
    pub data: Vec<u8>,
    pub mime: Option<String>,
}

pub fn read_track(path: &Path) -> Result<Track> {
    let tagged = Probe::open(path)
        .map_err(|source| CoreError::Tag {
            path: path.to_path_buf(),
            source,
        })?
        .read()
        .map_err(|source| CoreError::Tag {
            path: path.to_path_buf(),
            source,
        })?;

    // 采样规格来自容器 / 流头而不是标签。`> 0` 的过滤是把「库返回 0」当成「没读到」。
    let props = tagged.properties();
    let duration: Duration = props.duration();
    let audio = AudioInfo {
        sample_rate: props.sample_rate().filter(|rate| *rate > 0),
        bit_depth: props.bit_depth().filter(|bits| *bits > 0),
        bitrate: props.overall_bitrate().filter(|rate| *rate > 0),
    };
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let title = tag
        .and_then(Accessor::title)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });

    let artist = tag
        .and_then(Accessor::artist)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let album = tag
        .and_then(Accessor::album)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let has_cover = tag.is_some_and(|t| !t.pictures().is_empty());

    Ok(Track {
        id: TrackId::from_path(path),
        path: path.to_path_buf(),
        title,
        artist,
        album,
        duration: Some(duration).filter(|d| !d.is_zero()),
        has_cover,
        audio,
    })
}

/// 按需读取封面字节。只在真正需要显示时调用（当前曲目 + 列表可视区）。
pub fn read_cover(path: &Path) -> Result<Option<Cover>> {
    let tagged = Probe::open(path)
        .map_err(|source| CoreError::Tag {
            path: path.to_path_buf(),
            source,
        })?
        .read()
        .map_err(|source| CoreError::Tag {
            path: path.to_path_buf(),
            source,
        })?;

    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(None);
    };

    let picture = tag
        .get_picture_type(lofty::picture::PictureType::CoverFront)
        .or_else(|| tag.pictures().first());

    let Some(picture) = picture else {
        return Ok(None);
    };

    Ok(Some(Cover {
        data: picture.data().to_vec(),
        mime: picture.mime_type().map(ToString::to_string),
    }))
}
