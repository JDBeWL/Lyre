//! 歌单落盘到 `collections.json`：读不到退回空集，写不进只记日志。

use lyre_core::Collections;

use crate::persist;

const FILE: &str = "collections.json";

/// 读歌单。首次启动、文件损坏、目录不可读都退成空集而不是报错。
pub fn load() -> Collections {
    let mut collections: Collections = persist::load_or_default(FILE);
    collections.normalize_next();
    collections
}

pub fn save(collections: Collections) {
    persist::save_lossy(FILE, collections);
}

#[cfg(test)]
mod tests {
    use lyre_core::{CollectionId, Collections, TrackId};

    /// 落盘形状是 `{ next, items }` 而不是裸数组：`next` 那个计数器必须活下来，
    /// 否则重启后第一个新歌单会顶用老 id，把导航历史里还没退出的条目指到别处去。
    #[test]
    fn 计数器跟着一起往返() {
        let mut lists = Collections::default();
        let first = lists.create("深夜写歌").expect("建一").id;
        lists.get_mut(first).expect("在").add(TrackId(7));

        let text = serde_json::to_string(&lists).expect("序列化");
        assert!(text.contains("\"next\":1"), "计数器要真的写进文件：{text}");

        let mut back: Collections = serde_json::from_str(&text).expect("反序列化");
        assert_eq!(back.get(first).expect("还在").name, "深夜写歌");
        assert_eq!(back.get(first).expect("还在").tracks, vec![TrackId(7)]);
        assert_eq!(back.create("二").expect("建二").id, CollectionId(1));
    }

    #[test]
    fn 读不懂就当没有歌单() {
        let parsed: Collections = serde_json::from_str("这不是 JSON").unwrap_or_default();
        assert!(parsed.is_empty());
    }

    #[test]
    fn 空对象退成默认集() {
        let parsed: Collections = serde_json::from_str("{}").expect("空对象应当可读");
        assert!(parsed.is_empty());
    }
}
