# Lyre

基于 [GPUI](https://gpui.rs) 的桌面音乐播放器。

![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)

## 功能

- **播放**：播放 / 暂停、上下曲、拖动跳转（悬停预览落点）、音量与静音。随机与循环是两条独立的轴，组合出四种行为
- **队列**：跟随你正在看的那个列表 —— 在歌单 / 专辑 / 艺术家里点歌，队列就是那个列表。面板显示真实播放顺序，点任意一首直接跳播
- **曲库**：原生文件对话框、拖放、命令行路径三种导入方式；同路径重导只刷新标签不重复入库；收藏、关注、自建歌单（可拖拽排序、可指定封面图）
- **浏览**：曲目列表（虚拟化）与专辑 / 艺术家网格（点击下钻 + 面包屑返回）。多艺术家按 `/` `,` 拆成每一个人；搜索多词 AND 收窄，支持中文输入法
- **歌词**：`.lrc`（含 GBK / GB18030 编码）与 `.ass` / `.ssa`，双语按「同一时刻两行」，ASS 的 `{\kf}` 驱动卡拉OK 逐字。`L` 展开全屏遮罩
- **续播**：退出时记下哪一首第几秒，下次启动装回同一位置，不自动开播
- **可撤销**：删除与清空 6 秒内可撤销
- 三档响应式断点，侧栏 / 主区 / 右面板各自折叠。26 条快捷键在设置页里可查

## 技术栈

- **语言**：Rust edition 2024，MSRV 1.98.1
- **UI**：`gpui-pre` 0.3.8（Zed main 的快照分叉）+ `gpui-component` 0.7.1（输入框 / 菜单 / 浮层）
- **音频输出**：`rodio` + `cpal`
- **解码**：`symphonia` 0.6 全格式
- **标签**：`lofty`；`.lrc` 的编码嗅探用 `encoding_rs`
- **数据**：`serde` / `serde_json` 落盘，`walkdir` 扫目录

## 技术路线

- **分层**：`lyre-core` 是纯领域逻辑，不依赖 UI 也不依赖音频，能脱离窗口和声卡单测。音频对象活在 `lyre-audio` 里，视图与状态在 `lyre-ui`，`lyre-app` 只剩入口和键位绑定。
- **音频独占一条线程**：cpal 的 `Stream` 不实现 `Send`，音频对象只能关在专属线程的函数体里。UI 通过 `PlayerCommand` / `PlayerEvent` 两条单向通道，加一份原子 `Snapshot`，跟它打交道 —— 进度条每秒读几十次不加锁。
- **解码链**：`SymphoniaDecoder → LockFreeSource → UniformSourceIterator → FadeIn → rodio`。有界队列把解码和文件 IO 挡在 cpal 回调线程之外，显式重采样，seek 重建音源时淡入抹掉爆音。
- **渲染按变化频率分层**：gpui 的脏标记会打到所有祖先，所以高频组件（进度条、音量）自己订阅 `PlaybackEntity`，只在换曲时变的部分订阅语义事件，不跟着 10Hz 空转。
- **布局不写死列数**：断点只决定「哪块区域在不在」，一行放几个由可用宽度反推，列数与卡宽同源计算。
- **gpui 只从一处引入**：全项目只有 `crates/lyre-ui/src/platform.rs` 允许出现 `gpui` 这个名字，其余文件一律 `use crate::platform::*;`。这条隔离由 CI 用一条 grep 断言，换分叉只改那一处。
- **无边框窗口的缩放**：上游 `gpui-pre-windows` 在隐藏标题栏后只注册了顶边热区，本仓库 vendor 并补丁了它的 `WM_NCHITTEST`，见 `vendor/gpui-pre-windows/PATCH.md`。
- **持久化**：7 份 JSON 落在用户数据目录，先写 `.tmp` 再 `rename`；读不到就退回默认、写不进只记日志，数据目录坏了也不该让播放器打不开。

## 组件

| crate | 干什么 | 规模 |
|---|---|---|
| `lyre-core` | 曲目模型、播放队列语义、分组、歌词解析、目录扫描 | ~3.8k 行 |
| `lyre-audio` | 解码链、播放线程、命令 / 事件通道、进度快照 | ~1.8k 行 |
| `lyre-ui` | GPUI 实体与组件、设计令牌、持久化 | ~21k 行 |
| `lyre-app` | 二进制入口、全局快捷键绑定 | ~150 行 |

## 快速上手开发

需要 Rust 1.98.1 以上。首次构建要编译整棵 GPUI 依赖树（888 个包），几分钟后增量很快。

```bash
cargo run -p lyre-app                # 启动
cargo run -p lyre-app -- <path-to-file>    # 启动并导入这个文件 / 目录
```

Rust 侧没有代码级热更新，改了 `.rs` 必须重编重启；Windows 又会锁住运行中的 exe。

```bash
cargo install cargo-watch --locked
cargo watch -x "run -p lyre-app"
```

提交前跑 CI 的三道门（别名在 `.cargo/config.toml`）：

```bash
cargo f    # fmt --all -- --check
cargo c    # clippy --workspace --all-targets -- -D warnings
cargo t    # test
```

还有两条常用：

```bash
cargo doc --workspace --no-deps                    # CI 不执行，intra-doc 链接坏了只有这里报
LYRE_DEBUG_IMPORT="<path-to-file>" cargo run -p lyre-app   # 换一批测试素材
```

用户数据在 `%APPDATA%\Lyre\`（macOS `~/Library/Application Support/Lyre`，Linux `$XDG_DATA_HOME/Lyre`）：
`library.json` `settings.json` `likes.json` `follows.json` `collections.json` `recents.json` `resume.json`，
外加 `covers/` 与 `logs/app.log`。想从零开始，删掉这个目录即可。

## 许可

MIT OR Apache-2.0 双许可，全文见 [LICENSE-MIT](LICENSE-MIT) 与 [LICENSE-APACHE](LICENSE-APACHE)。
Copyright 2026 JDBeWL。

第三方组件保留各自许可：`gpui-pre` / `gpui-component` / `cpal` 为 Apache-2.0，`rodio` / `lofty` /
`crossbeam` / `serde` 为 MIT OR Apache-2.0，`symphonia` 为 MPL-2.0，`walkdir` 为 Unlicense OR MIT，
内嵌图标数据派生自 Lucide v0.544.0（ISC）。
