# Project State

## Purpose and Boundaries
- Patchlook 是 Rust / GPUI / GPUI Kit 实现的本地 Git 桌面查看器，目标平台为 macOS、Windows、Linux；采用 MIT 协议，不依赖后台服务或浏览器插件。
- 产品当前只读：查看工作区、暂存区、分支和提交，不执行暂存、提交、checkout、fetch、pull 或 push；分支浏览使用本地已有引用，“改动”页始终对应实际工作区，外部改动通过手动刷新重新读取。
- 文本 diff 要求 UTF-8；支持的图片独立预览，其他二进制文件只展示变化信息。运行环境需要 PATH 中可用的 Git，构建与平台依赖见 `docs/development.md`。

## Current Architecture
- 显示名称为 `Patchlook`，Cargo package 与可执行程序名为 `patchlook`，应用标识为 `dev.patchlook.app`；`src/main.rs` 初始化窗口、主题、菜单和快捷键，`src/lib.rs` 导出 Git 与设置模块；`src/about.rs` 的窗口级 AboutPanel 复用 Kit Base Dialog 和 Presence，在应用内居中展示关于页／协议正文；淡出结束才卸载遮罩并恢复焦点，遵循系统减少动态效果设置，版本／仓库／协议标识取 Cargo 构建元数据。
- `src/git.rs` 的 `Repository` 封装 Git CLI，直接传递参数、不经过 shell；通过 NUL 分隔结果保留特殊路径，并关闭可选锁、外部 diff 和 textconv 等相关行为。
- `src/app.rs` 的 `Patchlook` 管理共享仓库信息与页面展示；`ChangeState` 分别持有工作区和历史文件的列表、选择、滚动、树、diff、错误及任务，`HistoryState` 管理 Repos／History 共用的提交、搜索与分页。只保留当前仓库的两组状态；已加载 Tab 切换不读 Git，历史预览按需加载一次，隐藏页的异步结果仍写回所属状态。切仓库或手动刷新成功后替换当前页并使另一组状态失效；浏览分支只重载历史。新列表及首个 diff 在后台准备并一次替换，失败保留旧内容。
- 文本 patch 包含完整上下文，由 Kit 的 `DiffFile` / `DiffState` 解析和渲染；布局、上下文折叠、代码选择和虚拟滚动依赖 Kit。图片经 `Repository::file_versions` 读取相同基准的字节，由 `src/image_diff.rs` 解码、缩放和渲染。
- `src/change_tree.rs` 的 `ChangeTree` 将变更构造成 Kit 树控件，压缩单一路径目录，按原始文件索引保存选择身份；同一路径的暂存／未暂存条目及行数仍各自独立。
- `src/preferences.rs` 以 Cargo 包名 `patchlook` 确定系统应用配置目录，读写 `preferences.json`，保存外观、diff 模式、语言、文件列表模式、行数显示、自动换行、提交说明开关、最近仓库、各仓库浏览分支和 Repos 侧栏高度比例；读取与显式保存分离，写入经临时文件重命名完成，首次启动使用新目录的默认设置。
- `src/file_icons.rs` 为目录树和平铺列表提供同一文件类型映射与嵌入资源；`assets/file-icons/` 是 Material Icon Theme 的 35 个 MIT 许可 SVG，随包附带许可证，未知类型使用通用文档图标。
- `src/strings.rs` 使用 `locales/*.json` 的九种语言资源，并与 Kit 共用当前 locale；`themes/*.json` 提供统一配色。窗口重新激活时重读系统语言，系统外观变化由订阅响应。
- `.github/workflows/desktop.yml` 在 main／PR 上执行三平台检查和开发打包；与 Cargo 版本一致的 v 标签触发 macOS Developer ID 签名、公证及票据验证，三平台通过后创建 Release 草稿。`scripts/package.py` 统一生成包名和图标，默认 ad-hoc，显式参数启用正式签名及钥匙串公证凭据；失败不会退回开发包。手动运行填写 `notary_submission_id` 时仅查询既有 Apple 提交状态与完成后的日志，不构建或上传。

## Project Conventions
- GPUI 统一通过 `gpui_kit` 导入，复用其控件与主题；界面文案集中于 `locales/*.json`，各语言键及占位符须一致。
- `Cargo.toml` 将 GPUI Kit 0.8.0 patch 到 fork 的固定 Git revision，`Cargo.lock` 随应用维护；当前锁定 `e2a084aedbd36212545cf6242cc1c58f4b834562`。补丁归属与升级步骤以 `docs/development.md` 的“GPUI Kit patches”为准，不提交本机 path override。
- Git 基准是产品契约：未暂存比较索引与工作区，已暂存比较 HEAD 与索引；根提交按新增处理，合并提交比较第一父提交，历史与搜索续页固定在首次加载解析出的分支 tip；关键词按说明子串（忽略大小写）或 SHA 前缀匹配，流式扫描完整可达历史，每页最多保留 100 条与一条前瞻。参见 `Repository::history_filtered` 与 `Patchlook::load_history_page`。
- 验证入口为 `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`；Git 测试使用独立临时仓库。构建或依赖升级另执行 `cargo build --locked --release`；交互与滚动体验验收使用 Release 构建，Debug 构建仅用于开发调试。
- 开发构建缓存沿用根 `target/`；dev 使用 `line-tables-only` 与 `split-debuginfo = "packed"`，test 继承 dev，增量编译保留。常用检查保持相同的测试依赖特性组合；改名或依赖来源迁移后的旧 Debug 产物可定期清理，步骤见 `docs/development.md` 的“Build cache”。

## Implementation State
### Implemented
- 仓库选择、最近仓库切换与本地／远程跟踪分支浏览；最近仓库按名称忽略大小写排序，同名按路径排序，访问不改变顺序。分支选择按仓库保存，不改变实际 HEAD、索引或工作区；Tab 切换保留选择、滚动、树展开状态、历史分页和搜索结果，Repos 与 History 不重复保存历史数据。Repos 的 Recent／Branches 默认等高，拖动结束保存比例，重启和窗口尺寸变化后按比例恢复。
- 工作区分类变更、每页 100 条的提交历史与仓库三栏概览；Load more 是滚动列表尾部条目；未筛选提交数超过 20 条时底部常驻单行搜索框，搜索结果较少仍保留，按 Enter 或搜索按钮查询完整分支历史，清空恢复普通历史；Esc 取消搜索框焦点并保留查询。列表标题高亮匹配词，忽略大小写并保留 UTF-8 边界；SHA 前缀命中不高亮。提交说明可选择复制，右侧顶部按钮可显示／隐藏底部说明区（含 SHA 与作者信息），默认显示，开关状态跨页面与应用重启保留；短 SHA 可悬停查看、点击复制完整 SHA，并原地显示 Copied（随界面语言）2 秒后恢复。覆盖空仓库、根提交、重命名、冲突和 worktree 的读取路径。
- 目录／平铺文件列表、目录展开收起、文件定位；左键先收起当前展开目录，否则选中父目录；右键先展开目录，否则选中下一可见条目；通过统一选中状态监听，鼠标、上下左右键及程序定位选中文件时均打开 diff。支持文件、折叠目录和列表总计的新增／删除行数；二进制文件不参与文本行数统计。目录树采用 22 px 紧凑行高，目录保持中性色；两种列表共用源文件类型图标并移除状态字母，平铺图标为 20 px、目录树为 16 px；文件名配色为修改／重命名蓝色、新增／复制绿色、删除灰色、冲突警示色。平铺列表根目录路径显示为 `.`。
- 目录树的移动／重命名条目显示 `← 来源`，来源相对于目标目录；同名移动只显示原目录，改名同时显示原文件名。该标注独立于行数统计开关，悬停及读屏提供九语言的完整“移自”说明。
- 并排／行内文本 diff、语法高亮、自动折行、同步横向滚动、选择复制、上下文逐处或全局展开收起，以及连续改动块计数；在 diff 详情内切换文件时保留已有标题和内容，直到替换结果就绪；Repos 从目录树进入另一文件时先清除旧文本／图片预览并显示目标文件的加载状态，同一文件已完成的预览仍直接复用；选中后等待 120 ms 再加载，连续选择取消旧任务，相同文件重复选中不重新加载。
- PNG、JPEG、WebP、GIF、BMP、TIFF、ICO、SVG 前后版本预览，支持尺寸／大小信息、透明棋盘格、等比例适应窗口、1:1、缩放与面板内滚动。
- 亮／暗／跟随系统外观、九种界面语言及跟随系统语言；文件列表模式、行数显示、自动换行和提交说明开关统一由 Preferences 保存，交互时落盘、启动时恢复。其他分栏尺寸、窗口位置大小、当前 Tab、选中项、滚动位置与搜索不持久化。可拖动侧栏、固定文件工具栏和虚拟滚动列表。

- 关于页由顶部信息按钮或应用菜单打开，包含内嵌 logo、版本、九语言简介、MIT 协议、GitHub 与问题反馈链接；弹窗按内容实际高度垂直居中，关闭按钮／Esc／遮罩关闭均淡出；协议正文在同一弹窗内离线阅读与复制，根 LICENSE 与文件图标许可随三平台安装包分发。

### In Progress
- [active] 准备 v0.1.1 安装包，包含当前界面、状态缓存、偏好和关于页改进；Cargo 版本已更新。下一步推送 main 与版本标签，通过现有三平台流程构建并验证 macOS 签名、公证及 Release 草稿附件。
- [paused] 提交说明局部高亮与实时拖选：按用户决定不修改 Kit Base 层，等待上游支持／修复后再评估升级。当前 `SelectableText` 缺少局部高亮 API，拖选状态变化未订阅窗口刷新；详情文字高亮尚未实现，拖选期间选区不重绘的问题保留。已有列表标题匹配高亮独立完成。

### Next
暂无已确认的后续范围。

## Known Problems and Risks
- P2 [risk]: 自定义 diff 布局、上下文交互和滚动修复依赖 Kit fork；升级时需核对上游已合入的修复，并保留消费方回归覆盖。证据：`Cargo.toml`、`docs/development.md` 的“GPUI Kit patches”、`tests/diff_layout.rs`、`tests/scroll_bounce.rs`。

## Open Decisions
暂无待确认的决策记录。

## Verification
- 2026-10-26，v0.1.0（`c51e0c8`）的 GitHub 运行 `40340977301` attempt 2 全部通过：三平台格式检查、Clippy、Rust 测试、4 项 Python 发布失败路径测试、Release 构建和打包。Windows 两处测试的目录路径比较已修正并通过 CI。
- macOS Developer ID 签名、公证、DMG／应用票据附加与校验、Gatekeeper 评估均通过。首次提交曾超过 CI 的 1800 秒等待上限，随后由只读查询运行 `40350428057` 确认 Accepted；不是 Apple 拒绝应用。
- v0.1.0 Release 草稿已生成并核对附件：macOS arm64 DMG／ZIP、Windows amd64 ZIP、Linux x86_64 tar.gz；尚未公开发布。手动公证查询已验证只查询既有提交，跳过构建和发布。
- 2026-10-28，格式、Clippy、46 项 Rust 测试、4 项 Python 打包测试及 Release 构建通过；覆盖 Git／搜索、选择防抖、Repos 目录树进入详情不显示旧文本／图片及待加载选择的重复进入、切换提交期间保留旧内容、已加载 Tab 无 Git 读取仍可切换、隐藏页异步结果归属、分支切换保留工作区、空搜索复用、提交说明开关不重载 diff、四项展示偏好默认值／落盘恢复、关于页入口／链接、居中与三种关闭方式的动画生命周期及焦点恢复、读取失败保留快照、预览错误不丢列表、Esc 失焦及分栏序列化／全新窗口恢复。macOS Release 已实测默认等分、两种列表类型图标与根目录路径，并核对跨 Tab 保留工作区文件、Repos／History 共用同一历史提交与 diff、放大的平铺图标；原生拖动工具报 `noWindowsAvailable`，未完成真实拖动后重启验收。此前历史分页、复制和高亮已实测。界面测试使用临时仓库与独立临时配置目录，已核对真实偏好文件不受测试影响；Release 重启已验证目录列表、行数显示、自动换行及隐藏提交说明恢复；关于页 logo、版本、居中布局、协议阅读及关闭已在 Release 预览核对。未量化帧耗时；Windows／Linux 未另做实机验收，Linux 包仍依赖构建系统动态库。
- 2026-10-27 按新缓存配置清理并重建 Debug，格式检查、Clippy 和 32 项 Rust 测试通过；本机 `target` 从 14.45 GiB 降至 5.74 GiB，增量缓存 176.7 MiB。重复测试编译与 Release 构建均直接复用产物，Release 二进制校验和不变。Windows／Linux 未验证本次构建缓存配置。

## Source Pointers
- `README.md` — 默认英文产品首页，包含产品标志、功能、安装、使用和快速构建入口。
- `docs/development.md` — 平台依赖、开发验证、打包、签名发布和 Kit 补丁维护步骤。
- `Cargo.toml`、`Cargo.lock` — Rust 版本要求、依赖来源和实际锁定版本。
- `src/git.rs` — 只读仓库接口、diff 基准、路径解析、行数统计与真实 Git 仓库测试。
- `src/app.rs` — 桌面状态、后台加载、页面与文件切换、界面回归。
- `src/about.rs`、`LICENSE` — 关于页、构建元数据展示与离线 MIT 协议正文。
- `src/change_tree.rs`、`src/image_diff.rs`、`src/file_icons.rs` — 目录树、图片预览、文件类型图标与资源的实现及测试。
- `src/preferences.rs`、`src/strings.rs`、`locales/`、`themes/` — 持久化、语言与共享外观。
- `tests/diff_layout.rs`、`tests/scroll_bounce.rs` — Kit 补丁的消费方布局、上下文与滚动回归。
- `.github/workflows/desktop.yml`、`scripts/package.py`、`scripts/test_package.py` — 三平台验证、签名公证、Release 草稿与失败路径回归；Secrets 配置见 `docs/development.md`。
- `assets/app-icon/` — 已选用的 `patchlook-v2.png` 为深灰底米白色分隔 P，1254×1254、带透明通道；macOS 打包从此源图生成 ICNS，已检查 16／32／48 像素缩略图。原版 `patchlook.png` 因小尺寸细节过密保留作对照；对应提示词记录内置 imagegen 来源。
