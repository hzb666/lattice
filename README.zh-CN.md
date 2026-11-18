# Lattice GPUI

本工作副本只保留 GPUI 桌面编辑器及其实际依赖。独立的 Tauri 宿主、未被 GPUI 使用的前端模块和旧宿主发布流水线已移除。上游基线：`leo1oel/lattice@2cce98b71370fcff4b0db9aa20439d88650302cd`。

## 目录

- `gpui/`：原生应用、编辑器、PDFium 预览、WebView 桥接、测试和补丁。
- `gpui/islands/`：LaTeX、Markdown、白板、表格、幻灯片、Agent 和数学页面。
- `src/`：GPUI 仍使用的 WebView 模块、类型、样式、翻译、插画和内嵌技能。
- `src-tauri/lattice-core/`：共享后端，继续保留上游路径。
- `src-tauri/templates/`：内嵌论文模板；同目录保留的身份配置、运行时输入和登录脚本也被 GPUI 使用。
- `scripts/`：GPUI 构建、验证、资源生成和可选运行时准备。
- `tools/open-slide-runtime/`：可选幻灯片运行时；其余保留的工具和研究环境文件属于测试样本。

## 构建

需要 Node.js 22.18+、pnpm 10、仓库固定的 Rust 工具链，以及包含 `latexmk` 的 TeX 发行版。Windows 还需要 MSVC、LLVM、CMake、Ninja 和 PowerShell 7，并将相关工具加入 `PATH`。

```powershell
pnpm install --frozen-lockfile
pnpm build
& .\gpui\target\release\lattice-gpui.exe '论文项目路径'
```

`pnpm build` 依次构建 WebView 页面和原生 Release 程序。Cargo 编译统一经过 `scripts/gpui-build.mjs`，复用共享缓存并下载 PDFium。可以设置 `LATTICE_GPUI_BUILD_DIR` 复用已有缓存。macOS 的程序名没有 `.exe` 后缀。

`pnpm check` 执行 WebView 类型检查和页面单元测试；`pnpm test` 执行保留的共享模块与脚本测试。原生验证和测试项目运行方法见 [docs/agent-validation.md](docs/agent-validation.md)。功能说明见 [gpui/README.md](gpui/README.md) 和 `gpui/docs/`。

Agent 和幻灯片运行时按需准备，基础 LaTeX 编辑与 PDF 预览不依赖它们。翻译目录还供原生 Rust 界面使用，不能按当前 TypeScript 文件自动清除未扫描到的消息。

## 许可证

保留全部上游许可证和署名。GPUI 及 Trellis 的现有许可条款见 [gpui/README.md](gpui/README.md#license)，另见 `LICENSE`、`NOTICE` 和 `THIRD_PARTY_NOTICES.md`。
