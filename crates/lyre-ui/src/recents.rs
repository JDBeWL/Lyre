//! 「最近播放」列表，落盘到 `recents.json`：读不到退回空表，写不进只记日志。
//! 与队列面板的「已播放」（`LibraryEntity::just_played`）是两回事：这一份跨重启留着给首页。

use lyre_core::TrackId;

use crate::persist;

/// 落盘文件名。
const FILE: &str = "recents.json";

/// 留多少首。首页一次最多要 8 张快捷卡 + 12 张横排卡，60 条够翻，文件也只有几 KB。
pub const RECENT_CAP: usize = 60;

/// 最近在前的 id 列表。曲目被移出曲库后 id 读回来查不到，由调用方丢弃。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recents {
    ids: Vec<TrackId>,
}

impl Recents {
    /// 读不到（首次启动、文件损坏、目录不可读）就退回空表。
    pub fn load() -> Self {
        Self {
            ids: persist::load_or_default(FILE),
        }
    }

    /// 写回磁盘。失败只记日志，不影响调用方 —— 数据目录只读不该让播放器用不了。
    pub fn save(&self) {
        persist::save_lossy(FILE, &self.ids);
    }

    /// 记一笔：同一首反复播放只挪到最前，不占两格。
    pub fn record(&mut self, id: TrackId) {
        self.ids.retain(|existing| *existing != id);
        self.ids.insert(0, id);
        self.ids.truncate(RECENT_CAP);
    }

    /// 最近在前的前 `limit` 个。
    pub fn newest(&self, limit: usize) -> &[TrackId] {
        &self.ids[..limit.min(self.ids.len())]
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{RECENT_CAP, Recents};
    use lyre_core::TrackId;

    fn id(n: u64) -> TrackId {
        TrackId(n)
    }

    #[test]
    fn replaying_moves_the_same_track_to_the_front() {
        let mut recents = Recents::default();
        recents.record(id(1));
        recents.record(id(2));
        recents.record(id(1));

        assert_eq!(recents.len(), 2, "一首歌听两遍不该占两格");
        assert_eq!(recents.newest(9), [id(1), id(2)], "最近播的排最前");
    }

    #[test]
    fn the_oldest_entries_are_pushed_out() {
        let mut recents = Recents::default();
        for n in 0..(RECENT_CAP as u64 + 10) {
            recents.record(id(n));
        }
        assert_eq!(recents.len(), RECENT_CAP);
        assert_eq!(recents.newest(1), [id(RECENT_CAP as u64 + 9)]);
        assert!(
            !recents.newest(RECENT_CAP).contains(&id(0)),
            "最旧的那首应当已经被挤出去"
        );
    }

    #[test]
    fn newest_never_runs_past_the_end() {
        let mut recents = Recents::default();
        recents.record(id(7));
        assert_eq!(recents.newest(50).len(), 1, "要得比有的多时不该 panic");
        assert!(Recents::default().newest(8).is_empty());
    }

    #[test]
    fn round_trips_through_json() {
        // 落盘形状是 `Vec<TrackId>`；形状一变，老用户的文件就读不回来了。
        let mut recents = Recents::default();
        recents.record(id(2));
        recents.record(id(1));

        let text = serde_json::to_string(&recents.ids).expect("序列化不该失败");
        assert_eq!(text, format!("[{},{}]", 1u64, 2u64));

        let parsed: Vec<TrackId> = serde_json::from_str(&text).expect("反序列化不该失败");
        assert_eq!(Recents { ids: parsed }, recents);
    }

    #[test]
    fn corrupt_file_yields_empty_list() {
        let parsed: Vec<TrackId> = serde_json::from_str("这不是 JSON").unwrap_or_default();
        assert!(parsed.is_empty());
    }
}
