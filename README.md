# Lattice GPUI

This checkout contains the GPUI desktop editor and its required dependencies. The independent Tauri application, its unused frontend modules and its release pipeline have been removed. Upstream base: `leo1oel/lattice` at `2cce98b71370fcff4b0db9aa20439d88650302cd`.

## Source layout

- `gpui/`: native application, editor, PDFium viewer, WebView host, fixtures and patched dependencies.
- `gpui/islands/`: LaTeX, Markdown, board, spreadsheet, slides, agent and math pages.
- `src/`: WebView modules, shared types, styles, translations, illustrations and embedded skills used by GPUI.
- `src-tauri/lattice-core/`: shared backend, kept at its upstream path.
- `src-tauri/templates/`: embedded project templates. The remaining identity metadata, runtime inputs and login script also serve GPUI.
- `scripts/`: GPUI build and validation tools, catalog generators and optional sidecar preparation.
- `tools/open-slide-runtime/`: optional slide runtime; other retained tool and research-environment files are test fixtures.

## Build

Install Node.js 22.18+, pnpm 10, the pinned Rust toolchain and a TeX distribution with `latexmk`. Windows also needs MSVC, LLVM, CMake, Ninja and PowerShell 7 on `PATH`.

```powershell
pnpm install --frozen-lockfile
pnpm build
& .\gpui\target\release\lattice-gpui.exe 'path\to\project'
```

`pnpm build` builds the WebView pages and the native Release binary. Compiling Cargo commands use `scripts/gpui-build.mjs`, which preserves the shared build cache and fetches PDFium. Set `LATTICE_GPUI_BUILD_DIR` to reuse an existing cache. On macOS the executable has no `.exe` suffix.

`pnpm check` checks island types and runs island unit tests; `pnpm test` runs retained shared-module and script tests. Native validation and scratch-project scenarios are documented in [docs/agent-validation.md](docs/agent-validation.md). Feature details are in [gpui/README.md](gpui/README.md) and `gpui/docs/`.

The Agent and slide runtime are optional and have separate preparation commands. They are not needed to edit LaTeX or view a compiled PDF.

## Licenses

Upstream licenses and attribution are retained. GPUI licensing, including the existing Trellis terms, is documented in [gpui/README.md](gpui/README.md#license); see also `LICENSE`, `NOTICE` and `THIRD_PARTY_NOTICES.md`.
