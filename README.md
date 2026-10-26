# Patchlook

基于 Rust、GPUI 和 GPUI Kit 的本地 Git 桌面查看器，面向 macOS、Windows、Linux。所有仓库读取在本机完成，不需要后台服务或浏览器插件。

## 功能

- 打开仓库目录（也可选择仓库内的子目录），保存最近打开的仓库列表，点击即可切换仓库。
- “仓库”页上方为最近仓库，下方为本地和远程跟踪分支；悬停显示切换查看按钮。按仓库记住所查看的分支，不执行 checkout，也不改变 HEAD、暂存区或工作区。
- 查看未暂存、已暂存、未跟踪文件和冲突；同一文件的暂存与工作区改动分别展示。
- 浏览当前 HEAD 或所选分支的提交历史，每页 100 条；“仓库”模式以三栏显示仓库／分支、提交历史、提交目录树和完整提交说明。目录可逐个或一键全部展开／收起，单一路径目录合并展示，并显示文件数量；点击文件进入 diff，返回后保留展开状态与文件位置。
- “改动”和“历史”的文件列表可切换目录／平铺形式，保留当前文件和 diff；与仓库页共用改动行数开关。开启后各文件显示新增／删除行数，折叠目录显示汇总，顶部总数与条目右对齐。工作区同一路径的已暂存／未暂存条目及计数保持独立；二进制文件单独标识，不计入文本行数。
- 根提交与空仓库可正常查看；合并提交显示相对第一父提交的改动。
- 并排／行内 diff、常见语言语法高亮、行内改动强调、选择复制、前后改动文件切换。
- 默认自动折行；可关闭折行，两侧仍平分可见宽度，长行在各自代码区内同步横向滚动。
- 未变更内容折叠为可点击的波浪分隔线，悬停只加粗线条；支持逐处展开或一键展开／折叠全部上下文。
- 变更行左侧显示红／绿竖线，行号同步着色，覆盖自动折行后的完整高度。
- 紧凑的一体标题栏和固定文件工具栏，无底栏。右上角主题按钮循环切换跟随系统／亮色／暗色，完整仓库路径保留在打开仓库按钮的提示中。
- 文件列表标题显示总新增／删除行数；未提交页按列表中的各个暂存、工作区、冲突和未跟踪条目累计，历史页统计所选提交（合并提交相对第一父提交）。二进制文件不计入文本行数。
- 亮色／暗色／跟随系统，保存外观与 diff 模式；跟随系统时响应运行期间的外观变化。
- 右上角语言菜单支持跟随系统、English、简体中文、繁體中文、日本語、한국어、Français、Deutsch、Español、Português (Brasil)。即时切换界面、菜单与 diff 提示，并保存选择；系统语言按偏好顺序匹配，未支持的语言使用英文，重新激活窗口时重新读取系统语言。
- 侧栏顶部使用“仓库／改动／历史”页签；文件和提交列表具有贴边滚动条，提交列表仅显示说明、作者和日期，所选提交信息位于 diff 底部。
- 文件标题显示连续改动块数量；切换文件时等待新内容就绪后一次替换，保留加载期间的标题和代码。
- 图片 diff 支持 PNG、JPEG、WebP、GIF、BMP、TIFF、ICO、SVG，左右显示对应 Git 版本、尺寸和文件大小；透明区域使用棋盘格背景，工具栏提供适应窗口、1:1 原始尺寸和缩放，放大后可在面板内滚动。
- 侧栏激活页签与列表使用相同背景，不显示横向分隔线；diff 标题和提交信息使用单物理像素分隔线。
- 可拖动侧栏分隔线。文件与提交列表、文本 diff 使用虚拟滚动。

首版专注只读。刷新按钮或快捷键重新读取外部改动；不执行暂存、提交、检出、拉取或推送。分支浏览仅使用本地已有引用，不自动 fetch；“改动”页始终读取实际工作区。除支持的图片格式外，其他二进制文件显示变化信息。文本 diff 需要 UTF-8。

## 开发运行

需要 Rust 1.95+、Python 3.11+（仅打包时）以及 PATH 中可调用的 Git。Git 是本机数据读取工具，不是另一个常驻服务。

```sh
cargo run --locked
cargo run --locked -- /absolute/path/to/repository
```

也可以直接运行编译后的程序，或通过界面的“打开仓库”选择目录。Windows 路径包含空格时请加引号。

### macOS

macOS 15+，安装 Xcode 及命令行工具。确保 `xcode-select -p` 指向有效的 Xcode 开发者目录。

```sh
cargo build --locked --release
python3 scripts/package.py
open "dist/Patchlook.app"
```

安装时打开 `dist/` 下的 DMG，将 `Patchlook.app` 拖入其中的 `Applications` 文件夹。打包脚本从 `assets/app-icon/patchlook-v2.png` 生成包含标准与 Retina 尺寸的 ICNS，并写入应用图标声明。

### Windows

Windows 10+，MSVC Rust 工具链，Visual Studio 2022 的“使用 C++ 的桌面开发”组件及 Windows SDK，CMake 和 Git for Windows。安装 Git 时选择让第三方应用能够从 PATH 调用 Git。

```powershell
cargo build --locked --release
python scripts/package.py
.\target\release\patchlook.exe
```

### Linux

在图形化 X11 或 Wayland 会话中运行，需要支持 Vulkan 的显卡驱动。以 Ubuntu 24.04 为例：

```sh
sudo apt-get install gcc g++ clang cmake pkg-config libfontconfig-dev \
  libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev libvulkan1
cargo build --locked --release
python3 scripts/package.py
./target/release/patchlook
```

打包结果写入 `dist/`。macOS 产物为 `.app`、ZIP 和 DMG，Windows 为 ZIP，Linux 为 tar.gz。默认 macOS 包使用 ad-hoc 签名供本地安装验证；版本发布流程使用 Developer ID 签名和 Apple 公证。Linux 包使用构建系统的动态库，不能视为适配任意发行版的通用包。

### GitHub 版本发布

普通 `main` 提交和 Pull Request 运行三平台检查与打包，不读取发布凭据。推送与 `Cargo.toml` 版本一致的标签（例如 `v0.1.0`）后，macOS 任务在临时钥匙串导入证书，签名时启用 Hardened Runtime 与安全时间戳，将 DMG 提交 Apple 公证，并为 DMG 和 ZIP 内的应用附加公证票据。三平台任务全部通过后创建或更新 Release 草稿，供人工检查后发布；不会覆盖已公开 Release 的附件。

在仓库 `Settings → Secrets and variables → Actions` 配置以下 Repository secrets：

| 名称 | 内容 |
| --- | --- |
| `MACOS_CERTIFICATE_BASE64` | 包含私钥的 Developer ID Application `.p12` 文件的 Base64 |
| `MACOS_CERTIFICATE_PASSWORD` | `.p12` 导出密码 |
| `MACOS_SIGNING_IDENTITY` | 完整的 `Developer ID Application: 名称 (TEAMID)` 证书名称 |
| `APPLE_ID` | Apple 账号邮箱 |
| `APPLE_TEAM_ID` | 付费开发者团队 ID |
| `APPLE_APP_SPECIFIC_PASSWORD` | 为公证创建的 App 专用密码 |

证书、私钥和密码不放入仓库。凭据缺失、签名失败、公证未通过或票据验证失败都会终止发布任务，不退回 ad-hoc 包。首次公证可能超过工作流中 30 分钟的等待上限，此时任务失败，Apple 仍会继续处理；依据日志中的提交 ID 查询结果后再重跑。

本地也可使用已安装的证书及通过 `xcrun notarytool store-credentials` 保存的凭据：

```sh
python3 scripts/package.py \
  --sign-identity "Developer ID Application: Name (TEAMID)" \
  --notary-profile patchlook-notary
```

配置完成后提交代码并推送版本标签。签名与公证只是发布验证的一部分，不代表其他平台已经完成实机验收。

## 快捷键

| 操作 | macOS | Windows / Linux |
| --- | --- | --- |
| 打开仓库 | Cmd+O | Ctrl+O |
| 刷新 | Cmd+R / F5 | Ctrl+R / F5 |
| 复制所选代码 | Cmd+C | Ctrl+C |
| 选择当前侧代码 | Cmd+A | Ctrl+A |
| 退出 | Cmd+Q | Ctrl+Q |

## 验证

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Git 测试创建独立的临时仓库，覆盖空仓库、暂存与工作区基准、未跟踪文件、二进制和空文件、中文／空格路径、重命名、字面路径匹配、根提交、合并提交、冲突、worktree，以及 HEAD 变化时的稳定历史分页。总行数测试覆盖暂存与未暂存同时存在、未跟踪文本、无末尾换行、二进制、重命名特殊路径和根提交。还验证了生成的 patch 能被实际 Diff 组件解析。图片读取测试覆盖暂存／未暂存、未跟踪、重命名、删除、根提交，以及磁盘文件已删除后的历史版本；格式测试覆盖上述八类图片和损坏数据。图片界面测试覆盖相同缩放比例、等宽面板、原始尺寸、滚动和适应窗口，文件切换测试覆盖图片与文本工具栏同步替换。分支浏览测试校验本地／远程引用、完整提交正文，并验证浏览前后 HEAD、索引文件和工作区内容保持不变；最近仓库测试覆盖去重、排序和独立浏览分支的保存。仓库页界面回归覆盖三栏边界、悬停按钮显示，以及文件 diff 返回提交概览；目录树测试覆盖路径压缩、目录计数、排序后的文件索引、逐项及批量展开折叠、文件定位、目录／平铺切换时保留选中项与 diff、文件行数和折叠目录汇总、统计右对齐，以及同一路径不同暂存状态的独立计数。滚动测试覆盖触控板在内容中间松手、小数滚动量、真实顶底边界回弹，以及折行并排／行内 diff 松手后的稳定位置。不会修改已有仓库。

2026-10-26 本机验证：macOS arm64 的 `cargo test`（13 项 Git 测试、3 项设置与语言匹配测试、1 项语言资源完整性测试、2 项目录树构建测试、1 项图片格式测试、1 项改动块计数测试、11 项界面回归测试）、Clippy（禁止警告）、格式检查和 Release 编译通过。界面回归覆盖长行折行、左右等宽、横向同步滚动、右侧文本选择复制、窗口缩放、波浪线展开、行号旁箭头逐段收起及全局上下文展开／折叠，以及切换文件时加载期间的标题与代码保留、侧栏边缘滚动条与缩放分隔线的点击边界。桌面实测还覆盖紧凑标题栏的页面切换、滚动时固定的文件工具栏、波浪线悬停、图标资源与亮暗主题、目录选择、暂存／未暂存区别、根提交与历史改动、并排／行内、JSON／Rust 高亮、1,200 行文件滚动和三种主题选择。

GitHub Actions 配置在三个系统上编译、运行测试并打包；工作流需要推送至 GitHub 后才会实际执行。本机运行成功不能替代其余平台验证。语言测试校验区域与简繁中文脚本匹配、系统语言优先级、选择持久化，以及全部语言资源的键和占位符一致性；界面回归逐一打开全部语言的菜单，检查菜单边界、文案刷新与当前文件保持。macOS 桌面验证了英文／简体中文／日语切换、diff 提示和原生菜单同步更新，以及重启后恢复所选语言。三端验收时应检查仓库选择、历史翻页、长 diff 滚动、代码复制、缩放，以及系统亮暗和界面语言切换。

## 结构

- `src/git.rs`：独立的只读 Git 读取和真实仓库测试。
- `src/app.rs`：桌面状态、后台加载及界面。
- `src/change_tree.rs`：提交目录树构建与展示，复用 Kit 树控件的展开、选择和虚拟滚动。
- `src/image_diff.rs`：图片解码、前后版本预览、缩放和滚动。
- `src/preferences.rs`：设置的读取与显式保存。
- `src/strings.rs` 与 `locales/*.json`：共享组件语言设置的界面文案和各语言资源。
- `themes/`：统一的亮暗配色、悬停反馈与暗色语法高亮主题。

设置以 `patchlook` 为应用目录名保存在系统配置目录下，文件名为 `preferences.json`。构建锁定 GPUI Kit 0.8.0 与 `Cargo.lock`；通过 Kit 统一导入 GPUI，不混用其他 GPUI 版本。

### GPUI Kit 补丁维护

GPUI Kit 的修改在独立 fork [worldtheater/gpui-kit](https://github.com/worldtheater/gpui-kit/tree/codex/diff-tool-patches) 的 `codex/diff-tool-patches` 分支维护。上游基线为 0.8.0 发布源码提交 `1676826d9d49b90f4d7b6808b69924f58796f6f1`，当前补丁提交为 `e2a084aedbd36212545cf6242cc1c58f4b834562`。

根 `Cargo.toml` 用 `[patch.crates-io]` 将 `gpui-kit` 指向 fork 的固定 `rev`；Kit 及其 Base、Component、Assets、Macros 从同一个 Git 提交解析，底层 GPUI 继续使用原版本。`Cargo.lock` 随应用提交。Cargo 自动获取源码，开发者和 CI 不需要本地相邻仓库或 submodule。

补丁按目的拆分为独立提交：

| 提交 | 内容 |
| --- | --- |
| `697e0e7e` | 只有边缘已经拉伸时，触控板松手才传递回弹速度。 |
| `085bc94c` | 未触及边界时完整消耗小数滚动量，避免浮点误差触发回弹。 |
| `634f5b54` | diff 面板按视口等宽布局；代码同步横向滚动，行号与分隔线固定；文本选择限制在可见区域。 |
| `dcc55bcc` | 波浪折叠线、逐段收起箭头、折叠状态查询，以及换行后的改动条与行号颜色。 |
| `9697b217` | 按源文本统计连续改动块，独立于折叠和布局。 |
| `e2a084ae` | 九种语言的 Diff 文案，以及语言资源变化后的重新编译。 |

升级时，从目标上游版本建立新的补丁分支，逐项迁移仍需要的提交。上游 `e692bcb7`、`cf5b8639` 已分别处理两项滚动缺陷，升级到包含它们的版本时应重新核对，避免重复保留。推送新补丁分支后，再更新应用的 `rev` 和锁文件，并运行格式检查、Clippy、`cargo test --locked` 和构建；`tests/diff_layout.rs`、`tests/scroll_bounce.rs` 保留消费方回归覆盖。

本地联调时，可临时将同一个 `gpui-kit` patch 项改成 `path = "/path/to/gpui-kit/crates/kit"`。联调结束后恢复 Git `rev` 并重新解析锁文件，不提交机器上的绝对路径。

GPUI Kit 文档：[安装要求](https://gpui-kit.com/docs/installation/)、[Diff 组件](https://gpui-kit.com/component/diff/)。
