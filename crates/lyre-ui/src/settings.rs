//! 可持久化的界面偏好，落盘在用户数据目录的 `settings.json`。
//! 读不到退回默认值，写不进只记日志 —— 不弹窗、不阻塞启动。

use serde::{Deserialize, Serialize};

use crate::persist;
use crate::shell::{Density, NavSection, PanelTab, SidebarState};

const FILE: &str = "settings.json";

/// 可持久化的界面偏好。播放模式、进度这类会话内的状态不在这里。
/// 音量是例外：每次启动都该是用户上次要的那个值，包括当时是不是静音。
// `#[serde(default)]`：缺字段用默认值补齐，否则新增设置项会让老配置文件整体失效。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub density: Density,
    pub default_view: NavSection,
    pub recursive_import: bool,
    /// 启动时右面板停在哪一页，`None` 表示收起。窄一档一律收起：断点优先于偏好。
    pub default_panel: Option<PanelTab>,
    /// 启动时侧栏的形态，`None` 表示跟随窗口宽度。拖窗口仍会自动调整。
    pub default_sidebar: Option<SidebarState>,
    /// 记住上次播放的位置，下次启动装回同一首的同一秒（落盘见 [`crate::resume`]）。
    pub resume_position: bool,
    /// 用户要的音量，取值 0..=1。静音时这里存的仍是静音前的值，见 [`Settings::muted`]。
    pub volume: f32,
    /// 退出时是否处于静音。
    pub muted: bool,
    /// 输出设备的稳定标识（见 `lyre_audio::AudioDeviceInfo::id`）；`None` 表示跟随系统默认。
    /// 标识是机器绑定的，换机器或重装驱动后可能失效，那时静默回退默认设备。
    pub preferred_device: Option<String>,
    /// 换设备时让旧输出流淡出、新流淡入，把重建链路的空窗盖掉。
    /// 默认关：两路短暂并存时可能有轻微的相位抵消，听感不合就还是硬切干净。
    pub smooth_device_switch: bool,
    /// 用 WASAPI 独占输出。默认关：独占会占住设备，其它程序一起听不到声音。
    pub exclusive_output: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            density: Density::Compact,
            default_view: NavSection::Tracks,
            recursive_import: true,
            // 默认值必须等于「加这几个字段之前的行为」，否则老用户的 settings.json 一升级就变了样。
            default_panel: Some(PanelTab::NowPlaying),
            default_sidebar: None,
            resume_position: true,
            volume: 1.0,
            muted: false,
            preferred_device: None,
            smooth_device_switch: false,
            exclusive_output: false,
        }
    }
}

impl Settings {
    /// 读不到（首次启动、文件损坏、目录不可读）或字段缺失时退回默认值。
    pub fn load() -> Self {
        persist::load_or_default(FILE)
    }

    pub fn save(&self) {
        persist::save_lossy(FILE, self.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use crate::shell::{Density, NavSection, PanelTab, SidebarState};

    #[test]
    fn defaults_are_the_conservative_choice() {
        let settings = Settings::default();
        assert_eq!(settings.density, Density::Compact);
        assert_eq!(settings.default_view, NavSection::Tracks);
        assert!(
            settings.recursive_import,
            "递归导入是默认 —— 音乐目录几乎总是分层的"
        );
        assert_eq!(settings.default_panel, Some(PanelTab::NowPlaying));
        assert_eq!(settings.default_sidebar, None, "侧栏默认跟随窗口宽度");
        assert!(settings.resume_position, "接着上次播是大多数人想要的");
        assert_eq!(
            settings.volume, 1.0,
            "默认满音量 —— 与加这个字段之前的行为一致"
        );
        assert!(!settings.muted);
        assert_eq!(settings.preferred_device, None, "默认跟随系统默认设备");
        assert!(
            !settings.smooth_device_switch,
            "默认硬切 —— 与加这个字段之前的行为一致"
        );
        assert!(
            !settings.exclusive_output,
            "默认共享输出 —— 独占会占住设备，不能默认打开"
        );
    }

    #[test]
    fn round_trips_through_json() {
        let settings = Settings {
            density: Density::Comfortable,
            default_view: NavSection::Albums,
            recursive_import: false,
            default_panel: Some(PanelTab::Queue),
            default_sidebar: Some(SidebarState::Icons),
            resume_position: false,
            volume: 0.35,
            muted: true,
            preferred_device: Some("wasapi:{0.0.0.00000000}.{abc}".to_owned()),
            smooth_device_switch: true,
            exclusive_output: true,
        };
        let text = serde_json::to_string(&settings).expect("序列化不该失败");
        let back: Settings = serde_json::from_str(&text).expect("反序列化不该失败");
        assert_eq!(settings, back);
    }

    #[test]
    fn an_old_file_still_parses_and_behaves_as_before() {
        // 缺的要补成升级前的行为，也就是整个结构体的默认值，而不是 `None` / `false`。
        let old = r#"{"density":"Compact","default_view":"Tracks","recursive_import":true}"#;
        let back: Settings = serde_json::from_str(old).expect("老文件不该因为多出来的字段读不回来");
        assert_eq!(back, Settings::default());
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let back: Settings = serde_json::from_str(r#"{"density":"Compact"}"#)
            .expect("缺字段时应当用默认值补齐而不是报错");
        assert_eq!(back.default_view, NavSection::Tracks);
        assert!(back.recursive_import);
    }

    #[test]
    fn corrupt_file_yields_defaults() {
        let back: Settings = serde_json::from_str("这不是 JSON").unwrap_or_default();
        assert_eq!(back, Settings::default());
    }
}
