# gpui-pre-windows 本地补丁

这个目录是 `gpui-pre-windows 0.3.10` 的 vendored 副本（上游 = `zed@08cbb88` 快照），由根
`Cargo.toml` 的 `[patch.crates-io]` 指过来。

**为什么补**：上游对无标题栏窗口只注册了顶边的缩放热区，左 / 右 / 下三边没有原生缩放能力。
应用侧兜底只能走 `Window::resize`，而它是 `SetWindowPos(.., SWP_NOMOVE)`、原点钉死，
拖左边会变成对边在动，表现为「反向拉伸」。

**改了什么**：只有 `src/events.rs::handle_hit_test_msg` 一个函数，把顶边判定扩成四条边 + 四个角。
命中时只返回 `HT*` 码，位置、尺寸、光标形状、DPI 全部交给 Windows。角的热区比边放宽
`CORNER_GRAB_SCALE`（2）倍，否则对角只剩一个 8×8 的方块，稍偏就退化成单向边缩放。

**换 gpui 版本时怎么重做**：

1. 从 `~/.cargo/registry/src/<index>/gpui-pre-windows-<ver>/` 重新复制 `src/`、`Cargo.toml`、
   `build.rs`、`LICENSE-*`，不要带 crate 自带的 `Cargo.lock` 与 `.cargo-ok`。
2. 在新版 `handle_hit_test_msg` 里重新贴上面那处改动。
3. 同步根 `Cargo.toml` 里 `gpui` / `gpui-platform` 的 `=x.y.z` 钉版本，以及本目录 `Cargo.toml` 的 `version`。

**许可**：上游 Apache-2.0（见 `LICENSE-APACHE`），本地修补不改变许可。
