# Codex Sentinel

Codex Sentinel 是一个本机常驻工具，用 Rust/Tauri 监控 Codex Desktop
线程状态，并通过 Telegram 做远程可见控制。核心原则只有一个：所有继续、恢复、
自定义指令都必须回到 Codex APP 的可见线程窗口内提交，避免隐藏的 app-server
后台续跑导致桌面端看不到真实进度。

## 现有功能

- 读取 `~/.codex/state_5.sqlite`，展示最近线程、标题、工作目录和 rollout 路径。
- 读取 `~/.codex/logs_2.sqlite` 与 rollout 尾部，识别最近可行动错误。
- 读取最近 assistant/agent 消息，桌面端和 Telegram 都能查看线程最后反馈。
- 打开 `codex://threads/<id>`，在 Codex APP 可见输入框发送继续或自定义指令；macOS 走 Accessibility/CoreGraphics，Windows 走 Win32 窗口、截图与可见键盘输入。
- 安装官方 Codex `Stop` hook，在任务停止事件里判断是否需要让 Codex 继续。
- 运行 Telegram daemon，提供配对、状态、线程列表、线程详情、一键继续、输入指令和推送提醒。
- Codex 线程正常完成时，通过 Stop hook 向已配对的 Telegram 会话推送最后反馈。
- 作为托盘 / 菜单栏常驻应用运行，关闭窗口只隐藏；托盘 / 菜单栏图标可打开面板、切换可见自动恢复或退出。
- 安装轻量 lifecycle helper，开机后跟随 Codex APP：Codex 启动时拉起 Sentinel，Codex 退出时关闭 Sentinel GUI 和 daemon。

## 远程控制

桌面端 `Telegram 机器人` 面板提供配对码，不需要手动查 Telegram id：

1. 粘贴 BotFather token。
2. 点击 `保存配置`，或点击 `开始配对` 让桌面端等待消息。
3. 在你的机器人里发送面板显示的 `/pair 123456`。
4. Sentinel 自动写入当前 `user_id` 和 `chat_id`。
5. 点击 `启动后台`，之后手机端即可操作。

Telegram 主菜单保持极简：

```text
当前线程
继续当前
新线程
选择线程
待恢复
状态
清除归档
```

进入线程详情后可以 `刷新反馈`、`继续`、`输入指令` 或 `删除线程`。
`清除归档` 会清理已经归档的线程记录，并把归档 rollout 移到当前用户的 macOS 废纸篓。

## 托盘 / 菜单栏

Sentinel 常驻系统托盘或 macOS 菜单栏。左键点击图标会打开控制台；菜单里保留这些动作：

- `打开控制台`
- `可见自动恢复`
- `清除归档`
- `退出 Codex Sentinel`

`可见自动恢复` 会同步写入 `~/.codex-sentinel/config.toml`，同时打开或关闭本地 watcher。
`清除归档` 会复用同一条控制队列清理已经归档的线程记录，并把归档 rollout 移到当前用户的 macOS 废纸篓。

## 跟随 Codex 启停

Sentinel 使用一个很轻的 `lifecycle` helper 跟随 Codex Desktop。它自身常驻，但只做进程检查：

- Codex APP 运行时，自动启动 `Codex Sentinel.app` 菜单栏界面和本地 `daemon`。
- Codex APP 退出时，自动停止 Sentinel 菜单栏界面和本地 `daemon`。
- 下一次 Codex APP 启动时，helper 会再次拉起 Sentinel。

macOS 使用 `launchd` LaunchAgent；Windows 使用当前用户的
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\local.codex-sentinel`，保持在用户桌面会话里运行。

安装或刷新开机自启动：

```sh
/Applications/Codex\ Sentinel.app/Contents/MacOS/codex-sentinel install-launch-agent
```

Windows：

```powershell
.\codex-sentinel.exe install-launch-agent
```

检查跟随状态：

```sh
/Applications/Codex\ Sentinel.app/Contents/MacOS/codex-sentinel lifecycle-status
```

Windows：

```powershell
.\codex-sentinel.exe lifecycle-status
```

## 错误处理策略

自动恢复默认开启。除重新授权类问题外，其余已分类错误都会使用 0-5 秒短退避，
并按类型限制重复次数，避免无限循环。

| 错误类型 | 处理方式 | 延迟 | 次数上限 |
| --- | --- | --- | --- |
| `429 Too Many Requests` / retry limit | 自动短退避继续 | 5s | 10 |
| `502 Bad Gateway` / `503 Service Unavailable` / 500 | 自动继续 | 3s | 8 |
| stream disconnected / response body decode error | 自动继续 | 3s | 8 |
| silent turn completion | 立即自动继续 | 0s | 8 |
| model at capacity | 自动继续，让 Codex 重试或换路 | 5s | 3 |
| 403 Forbidden | 自动继续少量次数，让 Codex 换路或明确阻断 | 5s | 3 |
| insufficient balance / provider pool unhealthy | 自动继续少量次数，让 Codex 换路或报告阻断 | 5s | 3 |
| stale tool/process/MCP path | 立即继续，并提示不要复用失效工具路径 | 0s | 5 |
| 内容安全表述拦截 | 立即继续，使用本机授权维护/防御/排障边界重写提示 | 0s | 3 |
| 401 Unauthorized / auth expired / MCP OAuth expired | 不自动恢复，提示人工重新授权 | 0s | 0 |

## Hooks

Sentinel 只安装低频 `Stop` hook：

```toml
[features]
hooks = true
```

安装方式：

```sh
codex-sentinel install-hooks
```

安装器会保留已有 `~/.codex/hooks.json` 处理器，清理旧 Sentinel 条目，写入备份，并添加：

```text
Stop -> codex-sentinel hook-stop
```

Hook 事件写入用户 home 下的 `.codex-sentinel/hook-events.jsonl`，包含来源、动作、延迟、分类结果和最后反馈快照。
Sentinel 会在短时间内按 `event_key` 抑制重复 Stop 事件；需要较长退避的事件不会让 Stop
hook 一直 sleep，而是交给 watcher、Telegram 或桌面面板稍后恢复。控制台会显示最近 Stop
Hook 事件，并检查安装的 hook 是否指向当前 Sentinel 可执行文件。正常完成事件会记录为
`completed`，并由轻量后台子进程发送带 `追加指令` 按钮的 Telegram 完成通知，不阻塞 Codex Stop hook。

## 可见桌面控制

macOS 可见控制需要 `辅助功能` 权限；`屏幕录制` 用于状态观测。Windows 不需要这组系统权限，要求是 Codex 窗口处于当前用户桌面会话内、可见且未最小化。

```sh
codex-sentinel open-desktop-permissions
```

如果系统设置里已经打开，但 Sentinel 仍显示未授权，通常是本地重新打包签名导致 TCC
记录陈旧；把 `Codex Sentinel` 的辅助功能开关关掉再打开，或移除后重新添加当前
`dist/Codex Sentinel.app`。

开发阶段默认使用 ad-hoc 签名，每次内容变化都可能让 macOS 重新识别权限。若要长期固定
辅助功能授权，可以用稳定证书打包：

```sh
CODEX_SENTINEL_SIGNING_IDENTITY="Developer ID Application: Your Name" ./scripts/package-macos.sh
```

## CLI

CLI 保留为诊断和打包验证入口，不作为主要交互面：

```sh
codex-sentinel status
codex-sentinel recoverable
codex-sentinel running
codex-sentinel daemon
codex-sentinel lifecycle
codex-sentinel lifecycle-status
codex-sentinel install-launch-agent
codex-sentinel continue <thread_id>
codex-sentinel append <thread_id> <prompt>
codex-sentinel new <prompt> [path]
codex-sentinel hook-status
codex-sentinel install-hooks
codex-sentinel desktop-control-status
codex-sentinel debug-visible-send-plan
codex-sentinel debug-thread-failure-state <thread_id>
```

## Build

macOS：

```sh
npm install
cargo test
npm run build:ui
./scripts/package-macos.sh
open "dist/Codex Sentinel.app"
```

Windows：

```powershell
npm install
cargo test
npm run build:ui
npm run package:windows
```

## GitHub Release

线上构建发布 macOS arm64 DMG 和 Windows x64 EXE：

- `main` 和 PR：运行 macOS arm64 CI、Windows CI，并构建平台 artifact。
- tag 或手动 workflow：构建 DMG 和 Windows EXE，并发布到 GitHub Release。

手动发布：

```sh
gh workflow run release-build.yml --repo lich13/codex-sentinel -f version=0.1.0
```
