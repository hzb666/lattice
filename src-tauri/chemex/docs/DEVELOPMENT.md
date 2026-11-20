# 开发说明

## 当前范围

Chemex 源码位于 Lattice 仓库的 `src-tauri/chemex`，由 GPUI 和 `lattice-core` 通过仓库内路径依赖共享。该目录保留独立 Cargo workspace、CLI 与领域库测试；用户界面、项目管理和论文增量编译属于 Lattice。

Chemex 的 workspace 可独立验证；项目管理、桌面界面和论文编译由 Lattice 提供。点击 Lattice 的 Build 会先保存打开的源文件，再准备 Chemex 图稿，最后由原有 `latexmk` 工作流生成完整论文 PDF；用户可继续直接用 LaTeX 编译已导出/缓存的矢量图。Chemex 不另建编辑器事务系统。

## 模块

| 模块 | 职责 |
| --- | --- |
| `chemex-cdxml/parser.rs`、`model.rs` | 只读层级索引、源字节范围、对象 ID 歧义诊断、ObjectTag |
| `chemex-cdxml/chemistry.rs` | 原始 Fragment/Node/Bond 投影及局部引用检查 |
| `chemex-cdxml/text.rs` | 展示文字与原 XML 字节范围映射 |
| `chemex-cdxml/fields.rs` | 识别占位字段、校验映射、生成局部补丁 |
| `chemex-cdxml/grouping.rs`、`field_layout.rs` | 按作者分组关联结构、保持独立编号的水平中心 |
| `chemex-chemdraw` | 原生导出能力边界、单页校验及 PowerShell STA 辅助脚本 |
| `chemex-cli/chemex.sty`、`tex.rs` | CDXML 图片引用、LaTeX counter 桥接、绘图收集和编号清单 |
| `chemex-cli/build.rs`、`export.rs`、`tools.rs` | 构建调度、原生转换、持久缓存、超时和日志 |
| `chemex-cli/structures.rs`、`structures.py` | 字段到 CDXML Fragment 映射、RDKit 结构分析及保守去重 |
| `chemex-cli/output.rs` | 新生成目录发布与失败中间文件保留 |

解析核心库没有磁盘写入或 UI 依赖。`DrawingDocument` 拥有源文本，公开只读切片，内部索引不暴露可变引用。`chemex-chemdraw` 只依赖通用 XML 和错误类型，不依赖化学语义解析或界面；不支持的化学对象不会阻断原生导出。

选择 `roxmltree` 是因为当前需要只读层级和原文范围，且已在本机缓存；不需要 XML Writer。节点的文本范围可能受相邻文本/CDATA 合并影响，因此字段定位按 `<s>` 原文映射，不能直接把解码后的字符串下标用于原始 XML。

## 字段契约

- 字段为 `[[@key]]` 或 `[[@key_前缀^后缀]]`；装饰形式的 key 不得包含 `_`。前缀/后缀可为空或含空格，替换只改 key 所在位置，装饰及其样式保留。
- 编号由论文中的 LaTeX counter、`\the<counter>` 格式命令和 `\setcounter` 控制。Chemex 在图稿引用位置收集字段，并把 TeX 展开的纯文本编号写入生成 CDXML。
- 默认 `compound` 在各图稿的 source-order registration 中首次遇到字段时推进，正文引用只读取 numbering pass 生成的映射、不抢先占号；prepared final pass 按相同映射重放，确保前插图稿后正文和图稿编号同步顺延。其他自定义 counter 保留由正文或图稿首次出现推进的行为。编号写入 ChemDraw 前必须展开为非空、无控制符和无 TeX 命令的纯文本。整篇论文或图稿变化后重新解析编号；图稿只有替换后的 CDXML 哈希改变才重导。
- key 是全局化合物身份；多个年份或章节可使用多个 counter，但同一个 key 不能跨 counter 使用。重置同一 counter 不会改变已分配 key 的编号，新化合物应使用新 key。
- 只替换已识别的普通文字对象；原子标签、未知对象中的文字和 ObjectTag 内的文字只读。
- 跨 `<s>` 样式片段的字段把替换值放在 key 的起始处，继承该处样式；语法标记移除，前后缀留在原有样式片段，保留所有样式元素。
- 单个字段占满整个文字对象时，视为独立编号：在生成副本设置 `CaptionJustification` 和兼容属性 `Justification` 为 `Center`。已有居中锚点保持；其他对齐使用原 `BoundingBox` 水平中心，基线 y 不变。
- 独立编号仅清除自身已过时的 `BoundingBox` 和 `LineStarts`，由 ChemDraw 按新文字重新测量。缺少有效 `p`/文字框或存在非零旋转时，给出 `field_alignment_unavailable` 并保留原排版，仍可替换编号。
- 其余替换严格限定在字段字符的原始范围。句子中的字段不强制居中；保留其他字节，包括实体写法、换行、注释、字体表、结构坐标、箭头和未知对象。
- 缺少值、字段格式错误或不能可靠映射的文本会使本次生成失败；原始源文本始终不变。
- 对含自定义 DTD 实体或未知嵌套样式内容的字段不猜测替换范围。普通对象仍可检查，不含字段的文档可原样返回。
- 输出会再次解析以检查 XML 完整性；这不替代 ChemDraw 应用兼容性检查。

`chemex.sty` 在 `graphicx` 加载图片时截获明确带 `.cdxml` 扩展名的字面路径。发现、编号和最终构建按相同顺序记录引用。最终构建把匹配的 PDF 传给 `graphicx`，沿用原有图片选项。`\chemexdrawing[counter]` 仍可指定自定义 counter。普通图稿无需定义计数器。

## 化学语义边界

Fragment 保持作者创建的容器边界，不等同于已识别的 Compound。NodeType、未知键级、原始属性和显示信息都保留；不做缩写展开、盐合并、构型推断或结构规范化。

原子引用缺失、重复或跨 Fragment 会产生诊断，不自动连接错误结构。数字对象 ID 只用于当前文件的对象定位，不作为永久化合物身份。

当前没有 Drawing Hash 或 Structure Hash；不得把 XML 字节、对象顺序或坐标哈希当作化学身份。

## 分组关联

在 ChemDraw 中把一个化合物的结构和独立编号字段组成 Group。检查报告的字段增加 `group_object` 和 `fragment_objects`，均为当前文档 `objects` 的索引，不是永久 ID。

使用字段最近的祖先 Group；结构可以处在该 Group 内没有标签的子组中。遇到有独立字段的更内层 Group，结构归该内层组，防止外层 Scheme 抢占。只关联顶层 Fragment，缩写节点内部的 Fragment 不单独绑定。

每组仅允许一个不同的 Compound key；同 key 的多个标签可以共存。不同 key 同组时报告 `ambiguous_field_group`，不关联任何 Fragment，文字替换仍可进行。未分组字段和句子内引用不参与结构关联。盐的多个顶层 Fragment 保留为列表，不推断它们的化学身份。

## 输入与发布

- 输入为 UTF-8，可带 BOM；其他 XML 声明编码拒绝处理。CDX 需先通过 ChemDraw 生成分析副本。
- 源文件上限 64 MiB，XML 节点上限 250,000。
- 允许 ChemDraw 常见的 DOCTYPE，但不提供外部实体解析器，不读取 DTD 地址或外部文件。
- CLI 相隔 100 ms 读取两次源文件并比较全部字节，再做 XML 解析；这是稳定性窗口，不是操作系统文件锁或持续监听。
- 输出暂只发布到新目录：在同一父目录的临时目录生成完整文件后重命名。目标已存在、输入变化或校验失败均不会覆盖旧产物。
- CLI 的 `build` 仅用于 Chemex 独立集成验证，成功后只发布论文 PDF；Lattice 原生 Build 保留自身 PDF、SyncTeX、日志和增量状态，不通过 CLI 再套一层编译。
- 不在核心库添加 sleep 或文件观察逻辑；后续由宿主调度稳定快照。

## 原生导出和编译

`chemex-chemdraw::prepare_svg_export` 接收私有构建目录，把 `drawing.cdxml` 再复制为新的 `chemdraw-input.cdxml`，写出内置辅助脚本并返回命令。CLI 负责执行、超时和日志。路径通过参数和子进程环境传递，不拼接进 PowerShell 代码。

辅助进程使用本机 Windows PowerShell 的 `-STA` 模式和实测 ProgID `ChemDraw_x64.Application`。运行时检查 `ExportDataTypes` 中的 `image/svg+xml`，再调用本机实测的 `Documents.Open`、五参数 `SaveAs`。只关闭自己新打开的副本，不调用应用 `Quit` 或文档集合 `Close`。

当前路径为 ChemDraw 原生 SVG → Inkscape `--export-type=pdf --export-area-page` → `drawing.pdf`。不经分子图重绘，不默认转位图。EPS 虽可导出，但本机样本出现 Arial 被写为 Helvetica，因此没有采用为默认路径。SVG→PDF 的字体嵌入和视觉比较已对当前单页样本实测。

独立 CLI 的转换缓存保存在主 `.tex` 目录下的 `.chem/drawing-cache/v1/`。Lattice 编译与其化学预览共用 `.research/cache/chemex-v1/assets/`，以编号替换后 CDXML 的 SHA-256 保存渲染 SVG 和 PDF；准备记录校验当前 Tex/样式/CDXML 指纹。未变化时直接使用持久注册表和图稿，省去 ChemDraw、Inkscape 与编号准备 pass；正文编译仍由稳定目录中的 `latexmk` 管理依赖增量。编号 CDXML 副本同样保存在文稿专属缓存中。不要把 `build` 目录里的 PDF 当作 LaTeX 输入源。

多页和跨页画布尚未验证，导出明确拒绝；解析和字段替换仍支持多页。`build` 从原稿位置解析 TeX 项目，把中间文件写入暂存输出目录；尚不接管完整多文件项目。`latexmk` 使用 `-norc -pdf -interaction=nonstopmode -halt-on-error -no-shell-escape`。工具分别通过 `CHEMEX_INKSCAPE`、`CHEMEX_LATEXMK` 或 PATH 定位，没有硬编码本机安装目录。

每个外部阶段的标准输出和错误分别保存，超时 120 秒，只终止当前启动的辅助进程，不终止 ChemDraw 应用。超时时临时副本可能仍在原生应用中打开，因此必须保留构建目录。Windows 路径按 UTF-16 去除旧工具不兼容的 verbatim 前缀，保留 Unicode 和 UNC 路径形式。

`chemex structures SOURCE.cdxml` 是独立分析 CLI，不参与编辑器或绘图导出。Chemex 解析器先将动态字段关联到唯一编号 Group 及其嵌套 Fragment；RDKit 负责结构解析，按其 `CDX_FRAG_ID` 属性映射回原 Fragment。uv 使用固定 `rdkit==2026.9.2` 和 Python 3.13，不修改 Chemex 或宿主 Python 环境。重复身份采用同版本 RDKit 的规范异构 SMILES 精确匹配；不做盐拆分、互变异构、质子化或结构规范化猜测。query atom/bond、未指定立体化学和任何解析/映射不确定性均不可自动判重。RDKit 只支持 CDXML 中分子/反应解析所需的基础功能，不代替完整 CDXML 解析。辅助进程失败时仅保留标准输出/错误日志，JSON 请求文件（含源 CDXML）会删除。

## 样本与验证

样本来源、修改范围和第三方许可证在 `crates/chemex-cdxml/tests/fixtures/README.md`。手工样本与 ChemDraw 来源样本明确区分。

测试分别检查对象/化学属性、精确 XML 补丁、LaTeX 编号桥接、结构映射与去重、命令行结果、原件保护和错误路径。设置 `CHEMEX_LATEXMK` 时，CLI 集成测试会实际运行 TeX Live 验证编号样式与重置；ChemDraw 保存往返和生产环境视觉对比仍需单独验收。

ObjectTag 的属性参考 IUPAC 保存的 ChemDraw SDK 文档：

- `https://iupac.github.io/IUPAC-FAIRSpec/cdx_sdk/ObjectTag.htm`
- `https://iupac.github.io/IUPAC-FAIRSpec/cdx_sdk/properties/ObjectTag_Persistent.htm`

文字对齐语义参考同一 SDK 的 `properties/2DPosition.htm`、`properties/CaptionJustification.htm` 和 `properties/Justification.htm`：`p` 是随对齐方式变化的基线锚点；ChemDraw 7 以后优先采用 `CaptionJustification`，不能只修改旧 `Justification`。

文档描述不能证明本机版本的行为；即使自定义 Tag 被保留，也不能证明其化学身份在编辑后仍然正确。

本机验证用的 Inkscape 1.4.4 便携版来自官方 Windows 64-bit 7z 包，保存在忽略目录 `.chem/tools/`；SHA-256 为 `c4dbd64a92628abe7d7316c43f9325396e4d48417866027ee3d993d4f5b54c6a`。没有修改系统 PATH 或安装全局依赖。正式分发所需的工具发现、打包和许可证整理尚未开展。
