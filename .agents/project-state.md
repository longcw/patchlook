# Project State

## Purpose and Boundaries
- Patchlook 是 Rust / GPUI / GPUI Kit 实现的本地 Git 桌面查看器，目标平台为 macOS、Windows、Linux；不依赖后台服务或浏览器插件。
- 产品当前只读：查看工作区、暂存区、分支和提交，不执行暂存、提交、checkout、fetch、pull 或 push；分支浏览使用本地已有引用，“改动”页始终对应实际工作区，外部改动通过手动刷新重新读取。
- 文本 diff 要求 UTF-8；支持的图片独立预览，其他二进制文件只展示变化信息。运行环境需要 PATH 中可用的 Git，构建与平台依赖见 `README.md` 的“开发运行”。

## Current Architecture
- 显示名称为 `Patchlook`，Cargo package 与可执行程序名为 `patchlook`，应用标识为 `dev.patchlook.app`；`src/main.rs` 初始化窗口、主题、菜单和快捷键，`src/lib.rs` 导出 Git 与设置模块。
- `src/git.rs` 的 `Repository` 封装 Git CLI，直接传递参数、不经过 shell；通过 NUL 分隔结果保留特殊路径，并关闭可选锁、外部 diff 和 textconv 等相关行为。
- `src/app.rs` 的 `Patchlook` 管理“仓库／改动／历史”页面、选中项、加载任务和错误；Git 读取及 diff 准备在后台任务中执行，结果回到 UI 更新。
- 文本 patch 包含完整上下文，由 Kit 的 `DiffFile` / `DiffState` 解析和渲染；布局、上下文折叠、代码选择和虚拟滚动依赖 Kit。图片经 `Repository::file_versions` 读取相同基准的字节，由 `src/image_diff.rs` 解码、缩放和渲染。
- `src/change_tree.rs` 的 `ChangeTree` 将变更构造成 Kit 树控件，压缩单一路径目录，按原始文件索引保存选择身份；同一路径的暂存／未暂存条目及行数仍各自独立。
- `src/preferences.rs` 以 Cargo 包名 `patchlook` 确定系统应用配置目录，读写 `preferences.json`，保存外观、diff 模式、语言、最近仓库和各仓库浏览分支；读取与显式保存分离，写入经临时文件重命名完成，首次启动使用新目录的默认设置。
- `src/strings.rs` 使用 `locales/*.json` 的九种语言资源，并与 Kit 共用当前 locale；`themes/*.json` 提供统一配色。窗口重新激活时重读系统语言，系统外观变化由订阅响应。
- `.github/workflows/desktop.yml` 定义三平台格式检查、Clippy、测试、Release 编译和打包；`scripts/package.py` 从 Cargo 包名和英文语言资源读取名称，将已编译二进制封装到 `dist/`，不执行签名、上传或安装。

## Project Conventions
- GPUI 统一通过 `gpui_kit` 导入，复用其控件与主题；界面文案集中于 `locales/*.json`，各语言键及占位符须一致。
- `Cargo.toml` 将 GPUI Kit 0.8.0 patch 到 fork 的固定 Git revision，`Cargo.lock` 随应用维护；当前锁定 `e2a084aedbd36212545cf6242cc1c58f4b834562`。补丁归属与升级步骤以 `README.md` 的“GPUI Kit 补丁维护”为准，不提交本机 path override。
- Git 基准是产品契约：未暂存比较索引与工作区，已暂存比较 HEAD 与索引；根提交按新增处理，合并提交比较第一父提交，历史续页固定在首条提交的 revision。参见 `Repository` 与 `Patchlook::load_more`。
- 验证入口为 `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`；Git 测试使用独立临时仓库。构建或依赖升级另执行 `cargo build --locked --release`。

## Implementation State
### Implemented
- 仓库选择、最近仓库切换与本地／远程跟踪分支浏览；分支选择按仓库保存，不改变实际 HEAD、索引或工作区。
- 工作区分类变更、每页 100 条的提交历史、完整提交说明与仓库三栏概览；覆盖空仓库、根提交、重命名、冲突和 worktree 的读取路径。
- 目录／平铺文件列表、目录展开收起、文件定位，以及文件、折叠目录和列表总计的新增／删除行数；二进制文件不参与文本行数统计。
- 并排／行内文本 diff、语法高亮、自动折行、同步横向滚动、选择复制、上下文逐处或全局展开收起，以及连续改动块计数；切换文件时保留已有标题和内容，直到替换结果就绪。
- PNG、JPEG、WebP、GIF、BMP、TIFF、ICO、SVG 前后版本预览，支持尺寸／大小信息、透明棋盘格、等比例适应窗口、1:1、缩放与面板内滚动。
- 亮／暗／跟随系统外观、九种界面语言及跟随系统语言；可拖动侧栏、固定文件工具栏和虚拟滚动列表。

### In Progress
暂无已约定且需要继续执行的任务。

### Next
暂无已确认的后续范围。

## Known Problems and Risks
- P2 [risk]: 自定义 diff 布局、上下文交互和滚动修复依赖 Kit fork；升级时需核对上游已合入的修复，并保留消费方回归覆盖。证据：`Cargo.toml`、`README.md` 的“GPUI Kit 补丁维护”、`tests/diff_layout.rs`、`tests/scroll_bounce.rs`。

## Open Decisions
暂无待确认的决策记录。

## Verification
- 2026-10-26，Patchlook 改名后的工作区通过 32 项测试、Clippy 禁止警告、格式检查及 macOS arm64 Release 构建。锁文件仅改变根 package 名，依赖版本和来源不变。
- macOS 新产物为 `dist/Patchlook.app` 和 `dist/patchlook-0.1.0-darwin-arm64.zip`；已校验压缩完整性、二进制一致性、执行权限、应用名称／标识及九种语言声明，并启动确认窗口和原生菜单显示 `Patchlook`。旧名称生成包已清理。
- Git、设置与语言、目录树、图片、diff 布局及滚动回归由现有测试覆盖；改名后未重新执行完整桌面交互验收。之前的桌面实测范围见 `README.md` 的“验证”。
- 三平台 CI 仅确认配置存在，未查询远端运行结果；Windows／Linux 实机表现未独立验证。打包产物未进行开发者签名或公证，Linux 包依赖构建系统动态库，不是通用发行版包。

## Source Pointers
- `README.md` — 功能边界、开发运行、平台依赖、历史验证记录与 Kit 补丁维护。
- `Cargo.toml`、`Cargo.lock` — Rust 版本要求、依赖来源和实际锁定版本。
- `src/git.rs` — 只读仓库接口、diff 基准、路径解析、行数统计与真实 Git 仓库测试。
- `src/app.rs` — 桌面状态、后台加载、页面与文件切换、界面回归。
- `src/change_tree.rs`、`src/image_diff.rs` — 目录树和图片预览的实现及测试。
- `src/preferences.rs`、`src/strings.rs`、`locales/`、`themes/` — 持久化、语言与共享外观。
- `tests/diff_layout.rs`、`tests/scroll_bounce.rs` — Kit 补丁的消费方布局、上下文与滚动回归。
- `.github/workflows/desktop.yml`、`scripts/package.py` — 三平台验证和本地打包流程。
