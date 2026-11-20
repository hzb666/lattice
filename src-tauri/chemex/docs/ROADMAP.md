# 开发进度

## Lattice monorepo 收纳

- Chemex workspace、领域 crates、测试、样稿、验证记录和本机工具现统一保存在 `src-tauri/chemex/`；GPUI 与 lattice-core 使用仓库内相对路径共享，无外部仓库依赖。
- 默认 `compound` 编号按图稿出现顺序分配，新增前置图稿会使后续图稿和正文引用同步重编号。TeX Live 2022 CLI 集成通过前插回归及最终 PDF 构建验证。
- Chemex workspace fmt、Clippy `-D warnings`、71 项测试通过；`cargo check --manifest-path gpui/Cargo.toml -p lattice-core --locked --offline` 通过。

## V-006：Lattice Build

- Lattice 核心实测用 ChemDraw、Inkscape 和 TeX Live 构建嵌套稿件，确认最终 PDF 嵌入矢量图、原始 CDXML 哈希不变。未改动时再次构建无转换或排版步骤。
- Windows GPUI Release 构建成功。隔离项目桌面日志记录冷构建 43.0 秒、排队的后续 Build 1.3 秒，`.fls` 确认论文读取了持久缓存内的图稿 PDF。完整桌面 smoke 场景超时。
- 样稿保留 `\includegraphics{../../test.cdxml}`，使用准备目录后可直接运行 `latexmk`。在 `.chem/paper-01/build/manuscript.pdf` 中确认正文、图和图注的编号均为 1、2，第二次 `latexmk` 报告无需重编。

## STRUCTURE-001：RDKit CDXML 分析

- Chemex CLI 增加 `structures SOURCE.cdxml`，只分析唯一编号 Group；嵌套 Group 中多个 Fragment 合并为组结构，其他绘图对象和未分组字段不猜测身份。
- 固定使用 `uv run --with rdkit==2026.9.2 --python 3.13 --no-project`；RDKit 负责 CDXML 基础分子结构、分子式和规范异构 SMILES，Rust 负责 Fragment 映射及精确重复检测。CLI 可通过 `CHEMEX_UV` 覆盖 uv 路径，Windows 超时会终止该辅助进程树，不影响 ChemDraw。
- 只允许完整且立体信息明确、无 query 的结构参与自动判重。盐的组分保留，不自动规范化互变异构体、电荷或盐形式。
- 已实测 RDKit `2026.09.2` 可解析自带 `mol1.cdxml` 和手工嵌套多 Fragment 样本，并返回 `CDX_FRAG_ID`。Chemex fmt、Clippy `-D warnings` 和 workspace 73 项测试通过；实际 CLI 双组分结构 smoke 确认分子式、规范 SMILES、重复关系及源文件不变。更广泛 ChemDraw、query/stereo 边界样本仍待补充。
- 实现完全位于 `src-tauri/chemex`；没有接入 GPUI/Lattice UI。

## V-005：CDXML 直引与持久转换缓存

2026-11-19 在 `chemex build` 中接入 `\includegraphics{图稿.cdxml}`，支持普通及带编号图稿。`build` 与 `export` 共用主 `.tex` 目录下的 SVG/PDF 缓存。

- 使用本机 ChemDraw、Inkscape 和 TeX Live 对单页 `test.cdxml` 副本完成原生转换与论文编译。新进程仅调整图片宽度后复用字节一致的绘图 PDF，即使 `CHEMEX_INKSCAPE` 路径无效也能编译。
- 将化合物编号由阿拉伯数字切换为罗马数字后重新导出。`pdftotext` 确认正文、图稿及图注均使用 `I`、`II`。原始 CDXML 与测试副本的 SHA-256 均未改变。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`cargo test --workspace --locked --offline` 均退出 0。64 项测试包含真实 TeX 直引，以及缓存失效和转换失败的回归验证。

## V-004：LaTeX 主导编号

2026-11-19 将计数器配置入口改为论文 LaTeX：通过 `chemex.sty` 声明绘图位置与正文引用；TeX discovery/numbering/final passes 在原文顺序中执行原生 counter 与 `\the...` 格式，并把纯文本编号回填到生成 CDXML。旧的 JSON 编号读取器、映射样例和 `compound-numbers.tex` 查表 API 已移除。V-001 至 V-003 是旧 JSON 工作流的历史验证记录，不代表当前 CLI 参数。

| 验证 | 结果 |
| --- | --- |
| TeX Live 2022 CLI 集成测试 | 退出码 0；覆盖编号样式修改、重置、第二组新 key 和原件保护 |
| `chemex build examples/chemdraw-paper.tex .chem/tex-counter-integration` | 退出码 0；ChemDraw 23.0.1.10 原生 SVG、Inkscape PDF 和单页论文 PDF 均生成；编号流为 `compound-001=1`、`compound-002=2` |
| Rust 验证 | workspace tests、Clippy `-D warnings`、fmt check、CLI build 均退出码 0 |

Chemex 核心提供 CDXML 动态字段和原生矢量导出；Lattice GPUI 提供桌面界面与增量论文构建。日期：2026-11-20。

## 已实现

| ID | 能力 | 验收证据 |
| --- | --- | --- |
| CDXML-001 | 层级索引、未知对象、ObjectTag 与原文范围 | `chemex-cdxml/tests/parsing.rs` |
| CDXML-002 | 常规原子/键属性及错误诊断，保持 Fragment 边界 | 同上；两份 ChemDraw 来源样本 |
| FIELD-001 | 普通文字中的化合物字段、跨样式及多处替换 | `chemex-cdxml/tests/fields.rs` |
| FIELD-002 | 同一编号映射生成 CDXML 副本与 LaTeX 定义 | 同上；`chemex-cli/tests/cli.rs` |
| FIELD-003 | Group 关联结构和独立字段；组歧义诊断 | `chemex-cdxml/tests/grouped_fields.rs` |
| FIELD-004 | 短编号保持水平中心及纵向基线 | 对齐回归测试；用户确认及 V-003 原生样本视觉检查 |
| CLI-001 | JSON 检查报告、稳定读取、新目录发布与原件保护 | `chemex-cli/tests/cli.rs` |
| EXPORT-001 | 原生 SVG 和矢量 PDF 单页导出 | `chemex-chemdraw`；V-003 本机 ChemDraw、Inkscape 实测 |
| TEX-001 | 同映射编译正文、图注和完整 Scheme | V-003 正常构建、重新编号、失败保留结果 |
| BUILD-001 | 辅助进程隔离、超时、日志与整体发布 | CLI 单元测试；V-003 实际编译失败退出码 12 |

## 后续

| ID | 工作 | 验收要求 |
| --- | --- | --- |
| CDRAW-001 | 占位字段与 ObjectTag 的编辑保存往返 | 打开生成副本已验证；仍需记录编辑保存后的元数据保留行为 |
| EXPORT-002 | 扩大导出保真样本与多页支持 | 更多字体、特殊对象和多页逐项验证，未验证场景不能默默截断 |
| TEX-002 | 论文项目集成 | 多文件 LaTeX、其他图片和参考文献按项目边界构建 |
| SYNC-001 | 保存监听与构建调度 | 稳定快照、事件合并、旧任务结果不会覆盖新结果 |
| IDENTITY-001 | 扩大结构身份边界回归样本 | 真实 ChemDraw query/stereo/salt 样本验证保守判重不误判 |
| REGISTRY-001 | 永久 Compound/Occurrence UUID 和注册表 | 人工确认复用；冲突、合并、拆分可追踪 |
| LATTICE-001 | 接入已验证的编辑器底座 | 沿用原有项目和构建能力；客户端 UI 留到后续阶段 |

## 验证记录

V-001：2026-11-16，本机 Windows、Rust/Cargo 1.95.0。依赖从本机缓存离线解析并生成 `Cargo.lock`。

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 退出码 0 |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 退出码 0，无警告 |
| `cargo test --workspace --locked --offline` | 退出码 0；34 项测试通过：解析 11、字段 16、CLI 7 |
| `cargo run -p chemex-cli --locked --offline -- render crates/chemex-cdxml/tests/fixtures/fields.cdxml examples/compound-map.json .chem/demo` | 退出码 0；生成 3 处字段引用和共享 LaTeX 定义 |
| `target/debug/chemex.exe inspect .chem/demo/drawing.cdxml` | 退出码 0；剩余字段 0、诊断 0 |

实际命令行演示前后比较原样本 SHA-256，相同。生成结果位于 `.chem/demo/`，检查报告位于 `.chem/demo-inspection.json`；均为忽略的派生文件。

上述检查不包括本机 ChemDraw 打开/保存、ObjectTag 保留、矢量导出、视觉比较或 TeX 编译。这些验收项仍保持待办。

V-002：2026-11-16，按用户反馈增加 Group 关联与独立编号居中。先用真实文字坐标复现旧版左端偏移，再实现修复；同日相同 Rust 环境。

| 命令/检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 退出码 0 |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 退出码 0，无警告 |
| `cargo test --workspace --locked --offline` | 退出码 0；46 项测试通过：解析 11、字段 16、对齐 6、分组 6、CLI 7 |
| `cargo build -p chemex-cli --locked --offline` | 退出码 0；更新 `target/debug/chemex.exe` |
| `target/debug/chemex.exe render test.cdxml examples/compound-map.json .chem/manual-alignment-01` | 退出码 0；替换 2 处，仍保留原有 6 项 GenericNickname 诊断 |
| 原件与副本检查 | 原始 SHA-256 不变；151 个无关对象字节一致；剩余动态字段 0 |

`test.cdxml` 中两个独立编号的基线锚点从 `112.20 211.80`、`252 210.60` 改为 `167.225 211.80`、`307.025 210.60`。只清除这两处旧文字框并明确居中，不移动任何结构。该次用户文件还没有 Group，因此当时结构关联只完成回归样本验证。

已读取 ChemDraw SDK 的文字位置和对齐定义；本次未执行 ChemDraw 原生打开/保存及视觉比较，不能把 XML 属性和单元测试视为视觉验收。

V-003：2026-11-17，完成单页 ChemDraw → SVG → 矢量 PDF → LaTeX 纵向闭环。环境：Windows、Rust/Cargo 1.95.0、ChemDraw 23.0.1.10、Inkscape 1.4.4、TeX Live 2022、latexmk 4.77。

本机先枚举 COM 实际暴露的导出类型，再用临时副本验证 `Documents.Open` 和 `SaveAs`。EPS 导出把 Arial 替换为 Helvetica，故默认采用保留 Arial 的原生 SVG 路径。初次集成发现 `\\?\` 路径导致旧 PowerShell 执行失败，已规范外部工具路径并补 Unicode 路径回归测试。

| 命令/检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 退出码 0 |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 退出码 0，无警告 |
| `cargo test --workspace --locked --offline` | 退出码 0；56 项测试：CDXML/字段 39、桥接 3、CLI 单元 7、CLI 集成 7 |
| `cargo build -p chemex-cli --locked --offline` | 退出码 0；更新可执行文件 |
| `chemex build test.cdxml examples/compound-map.json examples/chemdraw-paper.tex .chem/paper-01` | 退出码 0；生成绘图 PDF 及单页论文 PDF，图文编号均为 1、2a |
| 换用 `.chem/native-probe/renumbered.json` 构建 `.chem/paper-02` | 退出码 0；图中、正文、图注均更新为 4、5b；Scheme 保持独立编号 1 |
| 引用未登记化合物的错误原稿 | 退出码 1；latexmk 退出码 12；不发布 `.chem/paper-invalid`，保留阶段日志，paper-01 的 PDF Hash 不变 |
| `pdffonts .chem/paper-01/drawing.pdf` | ArialMT、Arial-BoldMT、Arial-BoldItalicMT 均嵌入，且有 Unicode 映射 |
| `pdfimages -list .chem/paper-01/drawing.pdf` | 无栅格图像对象 |
| `pdftotext` 与 `pdftoppm` | 检查编号、Scheme 引用及实际论文页面，无缺失文字或明显偏移 |
| 原始 `test.cdxml` SHA-256 | 整轮构建前后相同；6 项 GenericNickname 诊断未阻断导出 |

本轮用户样本已含 Group；`inspect` 确认两个独立字段分别关联各自的 Group 和 Fragment。

原生 PNG 参考图与 PDF 渲染图已分别查看，样本的结构、箭头、文本、颜色、图形和居中编号均保留；这不是逐像素一致性证明，也不能替代更多字体、特殊对象和多页的验收。没有验证 ObjectTag 保存保留，也未实现自动保存监听。

实际产物与日志保存在忽略目录 `.chem/paper-01`、`.chem/paper-02`、`.chem/native-probe`；失败构建保存在 `.chem/.chemex-45460-1794845775682072600.tmp`。
