# Lattice 上游同步记录

## 审阅截止

| 项目 | 记录 |
| --- | --- |
| 同步方向 | `upstream/main`（`leo1oel/lattice`）→ 本地 `chemex` |
| 共同祖先 | `2cce98b71370fcff4b0db9aa20439d88650302cd` |
| 最近逐笔审阅范围 | `650b134fb6d5968f2b27e34750059e5df83cb3ef` 之后，共 6 笔 |
| 已审阅至 | `5856572714327c6a22e3f92df42f952ed8c97488`，当前项目中新建模板稿件 (#1259/#1265) |
| 本地同步结果 | `0769a69d`，完成本轮代码回移 |
| 最近确认 | 2026-11-25，`git fetch upstream main` 后指向 `58565727` |

早期记录曾审阅至 `032f99db969bbaac2ca4760cc3fda191f583d692`。`032f99db..9d43d560ad` 中已确认的回移和待办列在下文。`9d43d560..650b134f` 的 56 笔已逐笔核对；本轮继续逐笔审阅 `650b134f..58565727`，五笔选择性纳入，一笔因本地已删除 Tauri 宿主而跳过。

## 待整合

| 上游提交 | 还需要处理什么 |
| --- | --- |
| `2e113b63` | **旧应用对话导入。** 随 Synara 升级评估。需适配本地状态目录，验证导入范围和重复导入行为。 |
| `1dac6a71` | **macOS TeX Live 权限交接。** 上游会递归转移 TeX Live 安装目录的所有权，并静默更新已交接的安装。需在 macOS 验证提权范围、安装路径和本地 TeX 设置。 |
| `9f3794a6` | **界面细节。** 按需摘取 GPUI 画笔默认值和切换语言后的搜索占位提示；不恢复旧 Web 白板、导出通知和 Tauri 下载监听。 |
| `f15c1589` | **Appearance 标签样式选项。** 上游新增 Filled/Outlined；本地保留 `frosted` 外观，先评估是否需要额外样式再决定。 |
| `3143cbfd`、`fbd53966` | **原生 macOS 拼写检查。** 先核对 Harper 和现有拼写功能的调用，保留 Windows 的检查能力。 |
| `da8545cd`、`c2463222`、`21d7c838` | **Linux Agent Guard。** 防护主体、环境传递和描述符修复需要成组适配，验证本地 Windows 和 macOS 隔离路径。 |
| `7dd4203b`、`627f25cc`、`16d1569d` | **插件作者 SDK 和示例插件。** GPUI 插件宿主与市场消费者已接入；SDK、打包器及参考插件属于独立作者工具，不影响安装/运行市场插件，暂缓引入。 |

较早的 `eb7f1a76` 横跨多个已分叉的 GPUI 文件，待按当前 Clippy 报告逐项对照。`5c6753f8` 的首次启动迁移和 `1559a443` 的用户指南涉及旧应用数据或上游网站，暂作独立候选。

## 本轮补充审阅

| 上游提交 | 结果 | 取舍 |
| --- | --- | --- |
| `7403f93b`、`b0946158` | `61020b09`、`7a62fb54` | Synara 固定到 1.6.2 fork `f87641c8`，并接入 Diff 换行和 `+ → Files and folders`；保留本地 Windows 隔离和编辑转发。快捷键 `repeat` 仍由单独适配处理。 |
| `5f9e3931` | `e27dd5d0` | 仅移植 GPUI Archived threads 路由、搜索、E2E 和中英文文案；不恢复 Tauri Settings UI。 |
| `bbdb4dd6`、`59dc3d68` | `623d1f7e` | Agent 在 GPUI 原生白板上查看和编辑形状；新建统一使用 `.excalidraw`，兼容旧 `.tldr` 请求名，非法请求即时返回。测试使用转换后的场景，保留原 `.tldr` 样例和本地手稿编辑链路。补齐 `1bb3d25a` 的空场景种子，并适配本地路径接口。 |
| 本地编译修复 | `5e6b9aad` | 去掉插件回移时重复引入的 `ModalLayer::priority`，保留本地实现与初始焦点逻辑。 |
| `35931fdd` | `edefde94` | 主题/插件 registry 优先读取 `lattice-writer.github.io`，再回退旧站点；保留签名、serial 回滚保护，并增加多源测试。GPUI 文档和 schema 已更新；本地已删除的旧 `docs/guide/` 发布指南保持删除。 |
| `2e39149c` | `074bee9e` | 给 GPUI 主题市场测试索引补上插件列表字段，匹配共享 `Index` 类型。 |
| `c704848e` | 已有 `15bd91ba` | 当前 `chemex` 已包含该 GPUI tree-row drag 等待布局修复；确认是等价本地提交，不重复 cherry-pick。 |
| `4cf86a3a` | 跳过 | 只删除已不存在的 `scripts/gpui-e2e-known-failures.json`；不恢复旧豁免文件。 |
| `650b134f` | `184d3cf8` | 选择性加入 GPUI 任意文件附件、共享文件读取与测试；保留本地 macOS 场景限定，不带回 Tauri 宿主。 |
| `938cf272` | `ff6c8a37` | 纳入 GPUI/Linux 的 PDFium 定位、资源目录与 CI 构建/测试/启动 smoke；只产出短期 unsigned artifact，不改发行配置。 |
| `785140cb` | `f4c20a72` | 纳入 minted 代码块 PDF 反向定位，保留本地段落空行回退。 |
| `065edcb9` | 跳过 | 上游的 Tauri 退役/原生接管发行流程不适用于已删除 Tauri 宿主的 ChemEx；其旧宿主 catch-up 迁移也不恢复。 |
| `24cf539e` | `0769a69d` | 纳入 Windows E2E 截图和临时 unsigned archive；冲突解决时保留本地标题栏窗口控制 API。 |
| `a2d78773` | `f557c3a9` | 与 `785140cb` 成组纳入 minted 源码匹配边界和测试，避免注释、相似代码块等误匹配。 |
| `58565727` | `34025a87` | 纳入当前项目中新建模板稿件、文件冲突保护、主文档设置和新建项目位置预览；旧 `docs/guide` 保持删除。 |

## 已整合

| 上游提交 | 本地提交 | 已纳入的改动 |
| --- | --- | --- |
| `7c0a855e`、`de5782e9`、`b8beee04`、`a5199493` | `16011143`、`9aaceda9`、`16fefe5f`、`c2773be4` | TexLab 缓存、格式和 Windows Clippy、含括号的终端路径、进程测试。 |
| `b3dd2c79`、`ae05a488` | `c7c2919b`、`7f9661f4` | Windows 复制路径用本地分隔符并去除长路径前缀。 |
| `2917675e` | `ed1fb388` | macOS Codex 写入冲突恢复，沿用本地 Agent 沙箱。 |
| `1bbf4bab`、`a216812a`、`e571479a` | `51fa9209`、`20d59cdb`、`f5d6d581` | GPUI 延迟绘制拼接、图元单次入场、前台 journal 延迟初始化。 |
| `4f8d39da`、`074f056c` | `b5bab08f`、`567ecb96` | GPUI 导入冲突按钮和后端测试，文件树隐藏 REVTeX 生成的 `Notes.bib`。 |
| `f6df20cf`、`095da8d1` | `6470d337`、`77bf55ac`、`b209d52b` | Overleaf 改名保留链接，选择并记住下载目录，保护已有文件。 |
| `1b880d22`、`b0196eee`、`0b6300b8`、`7db29bc7` | `0d912abc`、`c77824c4`、`454dc41d`、`ab02b0c8` | PDF 重建保持阅读位置、重复适宽、拆窗实时预览、Build 恢复 PDF 面板。 |
| `e9159ad3`、`ff83d7c4` | `d976210a`、`277ef050` | 隐藏 WebView 交还键盘，原生焦点与 WebView 同步。 |
| `2ae9018e`、`0ed3a94b`、`ae118100` | `b6314d27`、`fc9abd67`、`f0707b9e` | 缓存构建指示器、减少动画路径缓存、选择包含当前章节的 LaTeX 根文档。 |
| `01166a71`、`9fd8919b`、`06a39ad3`、`30d0d52e` | `4252de28`、`1d900b08`、`ed526576`、`12ba8468` | 历史对比长行换行、文档页签键盘焦点、模态 Tab 范围、工作区菜单和命令。 |
| `bdfb33bd` | `df7e70b2`、`86b8ae5b`、`c05910f3` | 原生确认框在 Settings 上接收点击和回车。 |
| `ae159169`、`35839b12`、`09642d83`、`6fb20d41` | `1d338ce7`、`1b34776d`、`084f0367`、`4b892ebf` | 测试 Clippy、测试超时、现有 Windows 脚本适配和 MiKTeX 缓存修复。 |
| `7403f93b`、`b0946158`、`5f9e3931` | `61020b09`、`7a62fb54`、`e27dd5d0` | Synara 1.6.2 pin、Diff/文件选择修复和 GPUI Archived threads；shortcut `repeat` 尚未进入 Synara 固定版本。 |
| `78f475be`、`1bb3d25a`、`32fcbe29`、`f95ad6b4` | `36646b77`、`58c3c2a2`、`eb3b0ad5` | 本地已有 `.tldr` 转换、GPUI 原生 Excalidraw 白板与图元补丁；不再移植整套白板实现。Agent 的后续工具仍见待整合项。 |
| `196de782`、`89a502a2`、`a1e9221d` | `a375dea8`、`b3fe3874`、`71788a56` | 闲置 Agent 页面休眠、当前 pin 的嵌入输入框圆角、输入法开启时的快捷键。 |
| `84e1fd13`、`b3c9bf1b` | `cc443641`、`a9714462` | GPUI 键盘、预览焦点与重绘测试，关闭查找替换字段的自动更正。 |
| `c6058326`、`5b0d32de` | `cc6eef33`、`cd625125` | Papers 识别宏引用的参考文献库，原生最近项目搜索和标记。 |
| `9bdbb935`、`3f8c44ae`、`e2f1d53e` | `e49c36f2`、`e4b67541`、`9ee56fec` | 焦点提示高度、实时预览字体回退、通用文件图标。 |
| `7af4d829`、`a378b713`、`f0c27921` | `c7d58417`、`599979da`、`77828b0f`、`434c92eb` | Overleaf 句子级合并、拒绝文件名、CRLF 冲突标记和原生状态报告。 |
| `889c9b67`、`fd2e890d` | `177076ad`、`70ad9c84`、`1b26349c` | 切项目交还实时文档、checkpoint 测试、切换中阻止 join。 |
| `db5dabe5`、`a2859b13`、`5da68f34`、`16c66a1b` | `8eb604da`、`f490e4c7`、`ce061399`、`aead5f26` | Agent 拆窗、显示器位置恢复、Agent 字号、TeX 发行版诊断与平台快捷键。 |
| `2ad92fe4`、`63a934a5`、`7f09d4f7`、`8da84d16`、`6e0448b1`、`8b8cac9a` | `a101dbd1`、`c86391bb`、`fd30862a`、`0e57931c`、`0d9c561b`、`69d8440d` | 原生文件树/WebView 输入修复、共享表格右键、Markdown 表格列宽和行高、拖动恢复和撤销分组。 |
| `b2876c8d`、`addbb73e`、`76558961` | `ab00dfeb`、`e97a0625`、`1c01ef90` | 共享编辑器行内公式选择，rtex 退出恢复、首屏超时与安装校验。 |
| `71815871`、`737856b6`、`91bcd449` | `e20b7fe6`、`8f014c5d`、`432bb028` | 原生 WebView 光标与输入转发、代码文件语法高亮。 |
| `1083710c`、`410b8da6`、`63d66fc3`、`1c90977e` | `b190f053`、`5a5f5068`、`4f39516c`、`7b445fe8`、`0ed8cdf0` | Typst 编译、预览和 PDF 双向跳转，文件夹创建、构建耗时与关闭空面板。 |
| `4382c23e`、`7122e4ce` | `f77c1fbb`、`1853904e` | 共享快捷键长按控制、GPUI 和 WebView 的重复键处理。Synara iframe 的 `repeat` 转发仍待完成。 |
| `8631c316`、`97774795`、`c556b85b`、`073cbd19`、`f2b99888` | `c474d891`、`891f8541`、`4fd97ee0`、`f6b10ec8`、`3d21a63d` | GPUI 主题文件/解析器、签名 registry、共享熔断逻辑、主题市场及窄窗口按钮修复；保留本地面板风格，排除 Tauri 页面和样式。 |
| `032f99db`、`2eca5284`、`03bfae1b`、`c7a57df4` | `312aca34`、`69a516b4`、`446d29c3`、`7b961b3e` | 选择性加入沙箱插件宿主、GPUI 命令/设置、签名 registry 插件安装和 GPUI 市场；未恢复 Tauri host/IPC。 |
| `35931fdd`、`2e39149c`、`c704848e`、`650b134f` | `edefde94`、`074bee9e`、`15bd91ba`、`184d3cf8` | 注册表双源、GPUI 测试适配、拖拽布局修复和 Agent 文件附件；逐笔取舍见本轮审阅表。 |

## 已跳过

| 上游提交 | 取舍 |
| --- | --- |
| `469461b2`、`9856aff1`、`fd9080b0`、`60aca274`、`3e80a892`、`326e3824`、`79a68b10`、`63a0109b`、`8daa7f58`、`a8152ec1`、`44916d23` | 属于已停用宿主的窗口、PDF、Browser Host、旧同步提示或开发入口。原生实现按各自对应提交记录。 |
| `1bcb9f48`、`fcd7adf2` | 上游的更新源、链接和仓库切换。沿用 fork 的发行配置。 |
| `a91b5417`、`1299b79f`、`1ea3c21d`、`9701a30e`、`c5cff2bd`、`4276e863` | 上游专用的 CI 门禁、失败豁免、parity、脚本或体积预算。 |
| `f4408c63`、`d7f09af0`、`c4a364ac`、`a2a845a0` | 上游版本号、发布说明和旧宿主元数据。保留独立发行版本。 |
| `7304f7b5`、`145182ef`、`9d43d560ad` | 早期的启动预算、旧版本发布和上游 Mac runner 配置。 |
| `f2cd6d20` | 保留本地面板的 `frosted` 外观。 |
| `4a514a33`、`a0fde266`、`6fedc307` | Tauri 插件启用弹窗和旧市场页面；GPUI 市场已单独移植，不恢复 Tauri UI。 |

## 下一轮更新

```sh
git fetch upstream main
git log --reverse --oneline 5856572714327c6a22e3f92df42f952ed8c97488..upstream/main
```

逐笔读实际 diff，核对本地等价实现、前置依赖和被删除的路径。只摘取适用部分，保留 ChemEx、本地配置、Windows 隔离和现有工作区改动。整笔采用时使用 `git cherry-pick -x`。部分采用时在提交正文和本页注明来源及保留范围。

记录每笔的已整合、待整合或跳过结论，再更新截止 SHA、下一轮命令和本地结果 SHA。已审阅的待办保留在表中，避免因为推进截止点而遗漏。

本轮 Windows `cargo check --manifest-path gpui/Cargo.toml --locked --target-dir gpui/target -p lattice-gpui --tests` 通过；白板 `agent_canvas` 单测 4 项、`lattice-settings` 单测 21 项、附件和 Agent 文档协议 Vitest 各 5 项、`node scripts/gpui-host-errors.mjs --check` 均通过。未运行 registry Rust 单测和 macOS 原生场景，没有做全量翻译复验。
