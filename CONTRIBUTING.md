# 参与开发

欢迎反馈兼容性问题、修复 bug，或把使用说明改得和当前界面一致。较大的功能先开 Issue，写清场景和预期。

## 本地开发

安装 Rust、Visual Studio C++ 构建工具和 Windows SDK。本仓库的 `Cargo.lock` 是版本 4，需要能读这个锁文件的 Cargo。当前开发使用：

```powershell
rustup run 1.100.0-x86_64-pc-windows-msvc cargo build --locked --bin yunyu
rustup run 1.100.0-x86_64-pc-windows-msvc cargo test --locked --all-targets --features record -- --test-threads=1
```

给这次运行单独的数据目录，以免覆盖日常使用的 `%APPDATA%\Yunyu`：

```powershell
$env:YUNYU_DATA_DIR = Join-Path $env:TEMP 'yunyu-development'
rustup run 1.100.0-x86_64-pc-windows-msvc cargo run --locked --bin yunyu
```

日常测试用合成数据。带 `#[ignore]` 的真实账号测试和截图导出要显式运行。不要在公共 CI 里连接私人账号。

发布包用 `scripts/package.ps1`，需要 NSIS 3。安装目录、程序名和数据目录都是 Yunyu／`yunyu.exe`。Rust 包名和库路径都是 `yunyu`。

## 代码导航

| 目录／文件 | 内容 |
| --- | --- |
| `src/main.rs` | Windows 外壳、系统状态和后台服务 |
| `src/app.rs`、`src/pages.rs` | 状态与主界面。页面依次是音乐、专注、健康、置物架、待办、邮箱、AI 额度、日程、天气、第三方待办、会话 |
| `src/extras.rs` | 设置、通知队列、关于 |
| `src/calendar.rs`、`src/calendar_ui.rs`、`src/almanac.rs` | CalDAV、月历，以及离线农历和宜忌 |
| `src/mail.rs`、`src/mail_reader.rs` | IMAP、MIME、验证码和邮件阅读 |
| `src/quota.rs` 及各 provider 文件 | AI 额度与连接 |
| `src/todo_sync/` | 第三方待办同步 |
| `src/hotkeys.rs` | 全局快捷键 |
| `src/agent_session.rs`、`src/session_ui.rs` | 本机 Agent 会话的读取和会话页 |
| `src/pet.rs`、`src/pet_expressions.rs` | 吉祥物、日常表情和唱歌动作 |
| `src/offwork.rs` | 下班倒计时和专属表情时段 |
| `src/sys_meter.rs` | 常驻 CPU 与内存 |
| `src/weather.rs`、`src/qweather.rs` | Open-Meteo 与和风天气 |
| `src/native.rs`、`src/system.rs` | 原生窗口、输入、拖放和系统事件 |
| `src/ui_tests.rs` | 无窗口的界面回归测试 |
| `src/bin/yunyu-record` | 合成数据演示和软件渲染，需要 `record` 特性 |
| `vendor/` | 少量上游补丁，见依赖说明 |
| `installer/`、`scripts/` | 安装、卸载和打包 |

## 提交时注意

修 bug 时带上能复现的测试。改界面时附上不含私人信息的截图。个人配置、授权码、Cookie、编译产物和真实账号数据不要进仓库。

源码按仓库的 MIT 许可提交。第三方素材要一并写上来源和许可。vendored 依赖里的原始版权声明保留不动。
