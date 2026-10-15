//! 「关注」的艺术家集合，落盘到 `follows.json`：读不到退回空集合，写不进只记日志。
//! 键是艺术家名而不是 id：本仓库没有账户体系，关注只是本地记号。

use std::collections::HashSet;

use crate::persist;

const FILE: &str = "follows.json";

/// 用 `HashSet`：详情页与右面板每帧都要问「这位艺术家关注了吗」，`Vec` 的线性查找可测量。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Follows {
    names: HashSet<String>,
}

impl Follows {
    /// 读不到（首次启动、文件损坏、目录不可读）就退回空集合。
    pub fn load() -> Self {
        let names: Vec<String> = persist::load_or_default(FILE);
        Self {
            names: names.into_iter().collect(),
        }
    }

    /// 写回磁盘。失败只记日志，不影响调用方。
    pub fn save(&self) {
        // 集合无序，排序后写盘让文件在两次保存之间稳定。
        let mut names: Vec<&String> = self.names.iter().collect();
        names.sort_unstable();
        persist::save_lossy(FILE, &names);
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// 取反关注状态，返回切换后是否已关注。
    pub fn toggle(&mut self, name: &str) -> bool {
        if self.names.remove(name) {
            false
        } else {
            self.names.insert(name.to_owned());
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Follows;

    #[test]
    fn toggle_flips_and_reports_the_new_state() {
        let mut follows = Follows::default();
        assert!(!follows.contains("陈屿"));

        assert!(follows.toggle("陈屿"), "第一次切换应当变成「已关注」");
        assert!(follows.contains("陈屿"));
        assert_eq!(follows.len(), 1);

        assert!(!follows.toggle("陈屿"), "第二次切换应当取消关注");
        assert!(!follows.contains("陈屿"));
        assert!(follows.is_empty());
    }

    #[test]
    fn keys_are_distinct_artists() {
        let mut follows = Follows::default();
        follows.toggle("陈屿");
        follows.toggle("低苦艾");

        assert_eq!(follows.len(), 2);
        assert!(follows.contains("陈屿"));
        assert!(!follows.contains("林一"));
    }

    #[test]
    fn round_trips_through_json() {
        // 落盘形状是 `Vec<String>`；形状一变，老用户的文件就读不回来。
        let mut follows = Follows::default();
        follows.toggle("A");
        follows.toggle("B");

        let mut names: Vec<&String> = follows.names.iter().collect();
        names.sort_unstable();
        let text = serde_json::to_string(&names).expect("序列化不该失败");
        assert_eq!(text, r#"["A","B"]"#);

        let parsed: Vec<String> = serde_json::from_str(&text).expect("反序列化不该失败");
        let back = Follows {
            names: parsed.into_iter().collect(),
        };
        assert_eq!(back, follows);
    }

    #[test]
    fn corrupt_file_yields_empty_set() {
        let parsed: Vec<String> = serde_json::from_str("这不是 JSON").unwrap_or_default();
        assert!(parsed.is_empty());
    }
}
