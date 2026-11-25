# Chemex

Chemex 是 Lattice 内嵌的化学绘图与 LaTeX 桥接模块，提供 **CDXML 解析、动态化合物字段、ChemDraw 矢量导出和论文构建准备**。GPUI 桌面客户端和常规论文编译由 Lattice 提供。

ChemDraw 保持绘图编辑职责；原始 CDXML/CDX 是权威源文件。输出采用原文局部补丁，不通过重新绘制分子生成图片。

## 当前能力

- 读取 UTF-8 CDXML，索引页面、分组、Fragment、Node、Bond、文字、图形、曲线、ObjectTag 及未知元素。
- 提取原子元素、电荷、同位素、坐标、文字标签，以及键端点、原始键级和显示属性。
- 发现 `[[@compound-id]]` 和 `[[@compound-id_ Scheme ^ product ]]`，保留装饰文本样式并同步生成 LaTeX 定义。
- 化合物编号直接使用 LaTeX counter、格式命令与 `\setcounter`；不需要 JSON 映射文件。
- 支持跨文字样式片段的字段、XML 字符引用、CDATA、中文及多页重复引用。
- 通过 ChemDraw Group 关联结构与独立编号字段；可选调用独立 RDKit 分析组内多个 Fragment 并报告分子式、规范异构 SMILES 和精确重复项。
- 独立编号在生成副本中居中，保留原文字框中心与基线；普通句子内的字段保留原排版。
- 保留字段及其必要对齐属性以外的原始字节；未知对象、原子标签和元数据文字不参与字段替换。
- 报告不支持的结构、无效属性和歧义对象 ID；这些诊断不阻止普通字段处理。
- 使用本机 ChemDraw 原生 SVG 导出整张绘图，经 Inkscape 转为矢量 PDF，再由 `latexmk` 编译论文。
- `build` 成功后只发布论文 PDF；失败时保留日志和中间文件，不覆盖已有结果。

## 运行

从 Lattice 仓库根目录运行独立工具测试；GPUI 和 `lattice-core` 共享 `src-tauri/chemex/crates`。Rust 工具链由 Lattice 管理。

```powershell
$env:CARGO_TARGET_DIR = (Resolve-Path 'gpui/target').Path
cargo run --manifest-path src-tauri/chemex/Cargo.toml -p chemex-cli -- inspect src-tauri/chemex/test.cdxml
cargo run --manifest-path src-tauri/chemex/Cargo.toml -p chemex-cli -- structures src-tauri/chemex/test.cdxml
New-Item -ItemType Directory -Force src-tauri/chemex/.chem | Out-Null
cargo run --manifest-path src-tauri/chemex/Cargo.toml -p chemex-cli -- render src-tauri/chemex/examples/chemdraw-paper.tex src-tauri/chemex/.chem/demo
```

在论文导言区配置 LaTeX 计数器，并在绘图出现的位置声明 ChemDraw 文件。默认 `compound` 计数器按 CDXML 图稿出现位置推进，正文 `\chemex{key}` 只读取图稿分配的编号；前面插入图稿后，后续编号与正文引用会一并重排。自定义计数器保留 LaTeX 的原生顺序和重置行为。重置后若要给新化合物重新编号，请为它使用新的 ChemDraw 字段 key：

```latex
\documentclass{article}
\usepackage{chemex}
\newcounter{compound}
\renewcommand{\thecompound}{\Roman{compound}}
\begin{document}
\includegraphics[width=0.8\linewidth]{src-tauri/chemex/test.cdxml}
Compound \chemex{compound-001} was converted to \chemex{compound-002}.

\setcounter{compound}{0}
\renewcommand{\thecompound}{\alph{compound}}
\includegraphics{appendix.cdxml}
\end{document}
```

`render` 生成 TeX 编号驱动的 ChemDraw 副本：

```text
.chem/demo/
├── drawings/drawing-001/drawing.cdxml
├── drawings/drawing-002/drawing.cdxml
├── chemex-figures.dat
└── chemex-numbers.dat
```

`\includegraphics{路径.cdxml}` 仍可直接引用 CDXML，支持 `width`、`height`、`trim` 等 `graphicx` 选项。Mnova 推荐用 `\chemn`，也可统一使用 `\chemd`；两者按扩展名识别 `.mnova`，省略参数时选择第 1 张，序号超出文件谱图数量时报错。多谱图文件用逗号分隔的键值参数选择谱图，并可按分子动态编号叠加分子式：

```latex
\chemn[spec=2,cmpd=product-17,x=78,y=12,size=8pt]{figures/Document 1.mnova}
% \chemd[spec=2,cmpd=product-17,x=78,y=12,size=8pt]{figures/Document 1.mnova}
```

`cmpd` 是 CDXML 中的 Chemex 字段 key；构建会按动态编号查找该结构的分子式。`x` 和 `y` 是 0 到 100 的百分比，分别相对谱图宽度和高度，从左上角定位分子式左上角；分子式会缩放并夹在原谱图图片范围内，不改变原图片在 LaTeX 中的位置或尺寸。每条谱图引用独立设置位置和字号，以便避开各自的峰；`size` 使用绝对 TeX 长度，如 `8pt`、`2mm`、`1cm`。旧语法 `\chemn[2]{...}` / `\chemd[2]{...}` 仍可选择第 2 张；CDXML 的 `\chemd[counter]{路径.cdxml}` 仍用方括号指定 LaTeX counter。

Mnova 由 MestReNova 脚本接口从私有文档副本导出所选谱图所在页的 PDF，再由 LaTeX 嵌入。Lattice Build、hover 预览和独立 Chemex CLI 的 PDF 缓存在项目 `.chem/cache/mestrenova/mnova-<key>.pdf`；ChemDraw 的 SVG/PDF 缓存在 `.chem/cache/chemdraw/`。两者都按源文件、谱图序号和渲染器指纹复用，且不修改或保存原始 `.mnova`。Windows 上按 `CHEMEX_MESTRENOVA`、`PATH`、已运行进程、系统安装记录的顺序查找可执行文件。TeX Diagnose 使用相同的定位逻辑，直接读取文件版本，不启动 MestReNova。

默认 `compound` 按图稿在 LaTeX 原文中的引用顺序编号。每个 key 首次作为图稿字段出现时推进计数器，`\chemex{key}` 只查号，可以写在图稿之前。图稿中的重复 key 复用编号。其他计数器通过 `\chemex[counter]{key}` 指定，仍由正文或图稿中的首次出现推进。格式使用 `\Roman`、`\roman`、`\Alph`、`\alph` 或自定义 `\thecompound`，重置使用原生 `\setcounter`。

一次引用多个化合物时，在 `\chemex` 参数中用逗号分隔 key；可选 counter 仍放在方括号中。默认两项用 `and` 连接，三项及以上使用逗号和末尾的 `and`：

```latex
\chemex{compound-001, compound-002} % 1 and 2
\chemex[scheme]{compound-001, compound-002, compound-003} % 1, 2, and 3
```

key 本身不允许逗号。可在导言区重定义 `\ChemexListSeparator`、`\ChemexListPairSeparator` 和 `\ChemexListFinalSeparator` 来调整列表标点或本地化连接词，例如中文文稿可设置为 `、`、`和`、`和`。不同 key 仍是独立化合物身份，不会按编号相邻关系压缩成范围。

父化合物和子化合物用动态字段直接表示，不需要在导言区预先声明。普通字段使用 `parent-key`；子化合物字段使用 `parent-key|child-key`。Chemex 在图稿中按子字段首次出现的顺序分配 `a`、`b` 等字母，并在生成的 ChemDraw 副本中渲染完整编号。

把标记原样输入 ChemDraw 的普通文字对象即可；这是 Chemex 构建时替换的文本占位符，不是 ChemDraw 内置的自动编号域，原始 CDXML 不会被改写。只需子编号时可省略父字段，首个 `parent-key|child-key` 会隐式创建父编号；只有还要显示父号时才加 `[[@parent-key]]`：

```latex
% ChemDraw text objects
[[@compound-7]]
[[@compound-7|isomer-a]]
[[@compound-7|isomer-b]]

% LaTeX prose references
\chemex{compound-7|isomer-a} and \chemex{compound-7|isomer-b}
```

章节编号与子编号组合使用 LaTeX counter 格式。`report` 或 `book` 文档可以按章重置 counter，并在 `\thecompound` 中加入章节号：

```latex
\newcounter{compound}[chapter]
\renewcommand{\thecompound}{\thechapter-\arabic{compound}}
```

例如第 2 章的第 1 个父化合物及其两个子化合物会在生成的 ChemDraw 副本中显示为 `2-1`、`2-1a`、`2-1b`。章节前缀属于父编号，子字母附加在完整父编号之后，不冲突。父、子 key 均须唯一；父子关系只支持一层，每个父 key 最多 26 个子项。

多个年份或章节可以使用多个 LaTeX counter；每个 ChemDraw 字段 key 仍代表同一个全局化合物身份，因此不同 counter 必须使用不同 key。默认 `compound` 按全部嵌套正文展开后的图稿顺序编号；自定义 counter 仍保留首次正文或图稿引用推进的行为。重复使用同一个 key 但指定另一个 counter 会让构建失败，避免生成含义不明确的编号：

```latex
% In the preamble
\newcounter{compoundmain}
\newcounter{compoundsupp}
\renewcommand{\thecompoundmain}{2024-\arabic{compoundmain}}
\renewcommand{\thecompoundsupp}{2025-\arabic{compoundsupp}}
% In the document body
\chemd[compoundmain]{figures/2024.cdxml}
\chemd[compoundsupp]{figures/2025.cdxml}
```

重置同一个 counter 后，继续引用原 key 仍得到原编号；要表示一个新的化合物，必须在 ChemDraw 中使用新的 key。

目标目录必须不存在，父目录必须已存在。需要重新生成时选择新的目录；CLI 不覆盖已有结果。源文件连续两次读取相隔 100 ms，内容不一致则拒绝本次处理。

## 在 ChemDraw 中测试

1. 用独立文字对象输入 `[[@compound-001]]` 或 `[[@compound-001_ Scheme ^ product ]]`，将它放在结构下方，按需要对齐文字框中心。
2. 同时选中该化合物的结构和字段，使用 ChemDraw 的 Group 命令组成一组，然后保存 CDXML。不同化合物各自成组；盐的多个片段可以同组。
3. 生成副本并在 ChemDraw 中打开 `drawing.cdxml`，检查短编号是否仍在原文字框中心。源文件保留占位符，后续继续编辑源文件。

例如在论文项目中使用自己的 `manuscript.tex`：

```powershell
$env:CARGO_TARGET_DIR = (Resolve-Path 'gpui/target').Path
cargo build --manifest-path src-tauri/chemex/Cargo.toml -p chemex-cli --locked
cargo run --manifest-path src-tauri/chemex/Cargo.toml -p chemex-cli -- render path/to/manuscript.tex src-tauri/chemex/.chem/manual-02
$report = cargo run --manifest-path src-tauri/chemex/Cargo.toml -p chemex-cli -- inspect path/to/drawing.cdxml | ConvertFrom-Json
$report.fields | Format-Table compound_id, text_object, group_object, fragment_objects
```

`structures` 报告带字段的化合物组及无法唯一关联的字段。命令通过 `uv` 在隔离环境运行固定版 RDKit Python 包 `2026.9.2` 和 Python 3.13；需要安装 `uv` 或设置 `CHEMEX_UV`，首次运行会下载 RDKit wheel。报告中的分子结构合并该组所有直接或嵌套 Fragment。`duplicate_of` 只标记规范异构 SMILES 完全相同且立体信息明确的结构；盐组分保留，不自动统一质子化态或互变异构体。解析失败、查询结构、未指定立体化学、未分组或有歧义的字段都不会自动判重；RDKit 失败日志保留在系统临时目录，输入 CDXML 不改写。

`group_object` 和 `fragment_objects` 是检查报告中的对象索引，可通过 `$report.objects[索引]` 查看 ChemDraw 对象 ID。未分组字段仍能替换，但没有结构关联；同组不同 key 会报告 `ambiguous_field_group`。旋转文字或缺少有效位置/文字框时，报告 `field_alignment_unavailable`，保留其原排版并继续替换。

## 导出及编译论文

需要 Windows、已注册自动化接口的 ChemDraw、Inkscape，以及包含 `latexmk` 和 pdfLaTeX 的 TeX 发行版。工具可放在 PATH，也可通过环境变量指定；设置 `CHEMEX_LATEXMK` 后，构建进程会自动把其所在目录加入自己的 PATH。

本机已验证 ChemDraw 23.0.1.10、Inkscape 1.4.4、TeX Live 2022 / latexmk 4.77。Inkscape 便携版保存在本地忽略目录 `tools/inkscape`；其他机器需自行提供该工具。

```powershell
$env:CARGO_TARGET_DIR = (Resolve-Path 'gpui/target').Path
cargo build --manifest-path src-tauri/chemex/Cargo.toml -p chemex-cli --locked
$env:CHEMEX_INKSCAPE = (Resolve-Path .\src-tauri\chemex\tools\inkscape\bin\inkscape.com).Path
New-Item -ItemType Directory -Force src-tauri/chemex/.chem | Out-Null

# 生成带编号的绘图副本、原生 SVG 和矢量 PDF
cargo run --manifest-path src-tauri/chemex/Cargo.toml -p chemex-cli -- export src-tauri/chemex/examples/chemdraw-paper.tex src-tauri/chemex/.chem/export-demo

# 同时编译论文
cargo run --manifest-path src-tauri/chemex/Cargo.toml -p chemex-cli -- build src-tauri/chemex/examples/chemdraw-paper.tex src-tauri/chemex/.chem/paper-demo
```

`build` 成功后只发布最终论文：

```text
.chem/paper-demo/
└── manuscript.pdf
```

打开 `manuscript.pdf` 检查正文、图注和图中编号。修改 `.tex` 中的 counter 样式或重置位置，或者在 ChemDraw 中修改源文件后，选择新的输出目录重新执行即可。

`chemex build` 和 `chemex export` 将 SVG 和 PDF 持久化到主 `.tex` 所在目录的 `.chem/cache/` 下，ChemDraw 使用 `chemdraw/`，MestReNova 使用 `mestrenova/`，按编号替换后的图稿内容复用。`export` 生成的目录可作为 `TEXINPUTS` 交给 `latexmk`，原稿仍引用 `.cdxml`。在 Windows 下将导出目录放在 `TEXINPUTS` 开头，并以 `;` 保留 TeX 的默认搜索路径。改动图稿或编号后需要重新准备导出目录。

Lattice 的本地 Chemex 集成样稿保存在 `src-tauri/chemex/.chem/paper-01/` 和 `paper-02/`；Build 会准备图稿并使用普通 Lattice `latexmk` 流程排版，准备记录和矢量图保存在论文项目 `.chem/cache/chemdraw/`，分子分析数据库保存在 `.chem/molecules.sqlite`。默认编号改为按图稿出现顺序，因此插入新图稿后，既有编号及正文 `\chemex{key}` 引用都会顺延更新。ChemDraw 与 TeX Live 必须可用；随仓库保留的 Inkscape 位于 `src-tauri/chemex/tools/inkscape/`，Lattice 通过 `CHEMEX_INKSCAPE` 使用它。

当前 `build` 从原稿位置解析相对 `\input` 和资源，但把中间文件写入临时目录；尚不承诺完整多文件项目、BibTeX 依赖发现或自定义构建脚本。输入文件必须是 UTF-8。单张 ChemDraw 导出暂限单页；Mnova 导出按选中谱图所在页输出，用户样本的多页视觉保真仍待实测；`inspect` 和 `render` 支持多页。

失败时命令返回非零退出码，并打印保留中间文件的 `.chemex-*.tmp` 目录。检查对应的 `chemdraw`、`inkscape`、`latexmk` 日志；不会发布半成品到目标目录。辅助进程超时为 120 秒，不终止 ChemDraw 应用；超时时原生应用可能仍打开临时副本。

## 工程

- `src-tauri/chemex/crates/chemex-cdxml`：无 UI、无文件写入的解析及字段核心。
- `src-tauri/chemex/crates/chemex-chemdraw`：Windows 原生 ChemDraw SVG 导出准备及独立 STA 辅助脚本。
- `src-tauri/chemex/crates/chemex-cli`：稳定读取、TeX 编号协调、构建调度、工具日志和生成结果发布。
- [开发说明](src-tauri/chemex/docs/DEVELOPMENT.md)：模块边界、字段规则与限制。
- [开发进度](src-tauri/chemex/docs/ROADMAP.md)：已实现能力、验证记录和后续工作。

```powershell
$env:CARGO_TARGET_DIR = (Resolve-Path 'gpui/target').Path
cargo fmt --manifest-path src-tauri/chemex/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/chemex/Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/chemex/Cargo.toml --workspace --locked
```

## 当前边界

尚未实现 CDX 转换、文件监听或永久 UUID 注册表；Chemex 核心没有独立桌面 UI，编辑器界面由 Lattice 提供。RDKit 只支持 CDXML 中分子与反应解析所需的基础部分，不是完整 ChemDraw 文档解析器。ObjectTag 可读取，但没有完成本机 ChemDraw 编辑保存后的保留验证。

2026-11-17 已用用户提供的单页样本完成原生导出、PDF 视觉比对及 LaTeX 编译，并验证重新编号与失败保留结果。该样本不能证明所有字体、特殊对象和多页文档的保真；详细证据见开发进度。
