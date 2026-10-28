# Project State

## Purpose and Boundaries
- Patchlook 是 Rust／GPUI Kit 实现的本地 Git 查看器，目标为 macOS、Windows、Linux，采用 MIT 协议，无后台服务。只读浏览工作区、暂存区、分支与提交，不执行暂存、提交、checkout、fetch、pull 或 push；外部变更通过手动刷新读取。
- 文本 diff 要求 UTF-8；支持的图片独立预览，其他二进制只展示变化信息。运行需要 PATH 中可用的 Git。

## Current Architecture
- Cargo 包／二进制为 `patchlook`，应用标识 `dev.patchlook.app`；`src/main.rs` 初始化窗口、主题、菜单和快捷键。
- `src/git.rs` 封装 Git CLI，参数不经过 shell；NUL 分隔保留特殊路径，关闭可选锁、外部 diff 和 textconv；全文件上下文使用 `i32::MAX / 2` 并固定 inter-hunk context 为 0，避免旧版 Windows Git 的 32 位整数溢出和重叠 hunk。
- `src/app.rs` 管理共享仓库与页面。`ChangeState` 分别持有工作区／历史的列表、选择、树、滚动、diff、错误与任务；`HistoryState` 管理 Repos／History 共用的提交、搜索和分页。只缓存当前仓库，隐藏页结果写回所属状态；已加载 Tab 切换不读 Git。切仓库／刷新成功后替换当前页并使另一组状态失效，浏览分支只重载历史。
- 新列表、统计与首个预览在后台一起准备，再替换可见内容；读取失败保留快照，预览失败不丢列表。文本 patch 含完整上下文，解析、选择与虚拟滚动由 Kit `DiffFile`／`DiffState` 提供；图片经 `Repository::file_versions` 和 `src/image_diff.rs` 读取、解码、渲染。
- `src/external_open.rs` 封装外部打开：macOS 使用 NSWorkspace／UTType，Windows 使用 Shell，Linux 使用 GIO。推荐程序携带系统图标，无法取得时用通用图标。普通文件打开工作区最新路径；删除条目读取对应 diff 的原版本到临时副本，由应用保留至退出，不恢复工作区。菜单捕获目标，切换加载期间禁用。
- `src/change_tree.rs` 压缩单一路径目录，按原始文件索引保留身份；同路径的暂存／未暂存项独立。`src/file_icons.rs` 共用文件类型映射，内嵌 35 个 Material Icon Theme MIT SVG，未知类型用文档图标。
- `src/preferences.rs` 在系统 `patchlook` 配置目录读写 `preferences.json`，读取与显式保存分离，写入通过临时文件重命名完成。
- `src/strings.rs` 管理九语言资源并与 Kit 共用 locale；`themes/` 提供统一配色，浅色沿用 Kit 默认语法配色，仅将正文和普通变量／属性／嵌入文本调为深灰，深色使用自定义低饱和度主题；Diff 正文读取主题的 `editor_foreground`（当前 Syntect 后端的 JSON 字段名）。窗口重新激活重读系统语言，系统外观通过订阅更新。
- `src/about.rs` 复用 Kit Base Dialog／Presence，在窗口内居中显示关于页和协议；淡出结束才卸载遮罩、恢复焦点，遵循减少动态效果设置。版本、仓库与协议标识来自 Cargo 元数据。
- `.github/workflows/desktop.yml` 在 main／PR 执行三平台检查和开发打包；匹配 Cargo 版本的 v 标签触发 macOS Developer ID 签名、公证和票据验证，三平台通过后创建 Release 草稿。`scripts/package.py` 默认 ad-hoc，正式签名失败不降级。手动工作流的 `notary_submission_id` 只查询既有 Apple 提交，不构建或上传。

## Project Conventions
- GPUI 统一从 `gpui_kit` 导入，复用控件与主题；界面文案集中于 `locales/*.json`，九语言键及占位符一致。
- GPUI Kit 0.8.0 固定到 fork revision `e2a084aedbd36212545cf6242cc1c58f4b834562`；维护 `Cargo.lock`，不提交本机 path override。补丁归属和升级步骤见 `docs/development.md`。
- Git 基准：未暂存为索引对工作区，暂存为 HEAD 对索引；根提交按新增，合并提交比较第一父提交。历史／搜索续页固定首次解析的分支 tip；关键词匹配提交说明子串（忽略大小写）或 SHA 前缀，流式扫描可达历史，每页最多 100 条加一条前瞻。
- 检查：`cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`；构建／依赖变更另做 `cargo build --locked --release`，打包测试为 `python -m unittest discover -s scripts -p 'test_*.py'`。Git 和界面测试使用临时仓库／配置；交互验收使用 Release。
- 缓存统一用根 `target/`，dev 为 `line-tables-only`＋`split-debuginfo = "packed"`，test 继承 dev，保留增量编译；常用检查保持同一测试依赖特性组合，清理步骤见开发文档。

## Implementation State
### Implemented
- 最近仓库按名称忽略大小写排序，同名按路径排序；访问不改变顺序。Recent 加／减号可添加、移除记录，移除后选择相邻仓库，最后一项清空内容，不删磁盘文件。浏览分支按仓库保存，不改变实际 HEAD；Changes 始终对应工作区。窗口顶部分支跟随视图：Repos／History 显示浏览分支，Changes 显示工作区分支，悬停说明两者含义；History 的提交列表标题和文件树根节点不再重复显示分支名。
- Repos 三栏概览、Changes、History 共用上述状态；Tab 切换保留选择、滚动、目录展开和搜索，历史预览按需加载一次。Repos 的 Recent／Branches 默认等高，拖动结束保存比例，重启／缩放窗口后恢复。
- 历史分页的 Load more 是尾部条目；未筛选提交超过 20 条时常驻搜索框，筛选后仍保留，Enter／搜索按钮查询完整分支，清空恢复普通历史，Esc 失焦。列表标题高亮匹配词并保留 UTF-8 边界，SHA 前缀不高亮。提交说明可复制、显示／隐藏；短 SHA 悬停、点击复制完整号，原地显示 Copied 两秒。
- 文件列表支持目录和平铺、统计、键盘定位。左键先折叠再选父目录，右键先展开再选下一可见项；所有选中文件行为均打开 diff。目录行高 22 px，目录图标 16 px、平铺 20 px；文件名修改／重命名蓝色、新增／复制绿色、删除灰色、冲突警示色，根目录路径显示 `.`。移动／改名项显示相对来源 `← 来源`，同名移动只显示原目录，悬停／读屏提供完整说明。
- 单文件文本 diff 支持并排／行内、语法与行内变化高亮、折行、同步横向滚动、选择复制、上下文展开／收起及改动块计数。顶栏显示文件类型图标与打开方式菜单（系统默认、推荐应用图标＋名称、其他程序），无整文件收起按钮。选择防抖 120 ms，连续选择取消旧任务；详情切文件保留旧预览至新内容就绪，Repos 目录进入另一文件先显示目标加载状态，已完成的同文件预览复用。
- PNG、JPEG、WebP、GIF、BMP、TIFF、ICO、SVG 前后预览，含尺寸／大小、透明棋盘格、适应窗口、1:1、缩放与滚动。
- 持久化外观、diff 模式、语言、列表模式、行数显示、折行、提交说明开关、最近仓库、各仓库浏览分支、Repos 侧栏高度比例；其他分栏尺寸、窗口位置大小、Tab、选中项、滚动和搜索不持久化。
- 关于页仅由应用菜单打开，含 logo、版本、九语言简介、MIT 正文、GitHub／反馈链接；关闭按钮、Esc、遮罩均淡出。根 LICENSE 和文件图标许可证随三平台包分发。

### In Progress
- [paused] 提交说明局部高亮与实时拖选：用户决定不改 Kit Base，待上游支持／修复后再评估。当前 `SelectableText` 缺局部高亮 API，拖选变化未触发窗口重绘；列表标题高亮已完成。

### Next
暂无已确认的后续范围。

## Known Problems and Risks
- P2 [risk]: diff 布局、上下文与滚动依赖 Kit fork；升级时核对上游合入情况，保留 `tests/diff_layout.rs`、`tests/scroll_bounce.rs` 的消费方回归。

## Open Decisions
暂无待确认的决策。

## Verification
- v0.1.2（`d76bfa0`）云端运行 `40584779645` 三平台格式、Clippy、测试、Release 构建和打包通过：macOS 51 项、其他平台 50 项 Rust 测试，各 4 项 Python 打包测试。该版本已公开发布到 GitHub Releases 并标为 Latest，含 macOS arm64 DMG／ZIP、Windows amd64 ZIP、Linux x86_64 tar.gz。
- macOS Developer ID／Hardened Runtime 签名、公证及票据验证通过；Apple 提交 `6648a532-b61d-440d-96d6-0ae371468683` 为 Accepted。本地下载至 `dist/v0.1.2/` 后核对 CI 摘要、版本、许可证、签名、应用／DMG 票据及 Gatekeeper，均通过，Release 资产摘要与本地一致。功能回归覆盖外部打开版本基准、程序选择与加载禁用、推荐图标、仓库移除、Git／搜索、页面状态、偏好及 About。
- Windows patch 上下文参数修正通过格式、Clippy 和本地 51 项 Rust 测试；回归覆盖 LF／CRLF、未暂存／已暂存／历史、极大 inter-hunk 用户配置。使用 Git 2.49 的 xemit 源码模拟 Windows 的 32 位 long，复现第 60 行重叠 hunk，安全参数生成的 patch 解析通过；尚未在 Windows 实机复验。此前 macOS Release 预览已确认分支标签、按钮配色、默认浅色语法颜色与深灰正文，并实测类型图标、Tab 复用、搜索、复制、偏好恢复、About、外部打开和应用菜单。真实拖动分栏后重启仍未验收；未量化帧耗时。
- Windows／Linux 已通过云端原生检查、测试、链接与打包，未做实机交互；CI／开发文档包含 GLib 开发依赖，Linux 安装包仍依赖构建系统动态库。
- 缓存优化时本机 `target` 从 14.45 GiB 降到 5.74 GiB，增量 176.7 MiB；重复构建复用产物。属于当时测量，非当前容量保证；其他平台未测。

## Source Pointers
- `README.md`、`docs/development.md` — 产品介绍、依赖、验证、缓存、签名发布及 Kit 补丁。
- `Cargo.toml`、`Cargo.lock` — 版本、构建配置及依赖来源。
- `src/git.rs`、`src/external_open.rs` — Git／文件版本契约、系统应用关联及回归。
- `src/app.rs`、`src/change_tree.rs`、`src/image_diff.rs`、`src/file_icons.rs` — 状态、界面、导航与预览。
- `src/preferences.rs`、`src/strings.rs`、`locales/`、`themes/` — 设置、语言和样式。
- `src/about.rs`、`LICENSE`、`assets/app-icon/` — About、许可证和正式图标源 `patchlook-v2.png`。
- `tests/`、`scripts/test_package.py`、`scripts/package.py`、`.github/workflows/desktop.yml` — 验证和三平台打包发布。
