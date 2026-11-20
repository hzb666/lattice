# Lattice GPUI

This checkout builds the GPUI desktop editor. The independent Tauri host and its release pipeline have been removed.

## Ownership

- Native UI, source editor, PDFium and WebView bridge: `gpui/crates/`.
- WebView entrypoints: `gpui/islands/`; shared implementation: `src/`.
- Backend and templates: `src-tauri/lattice-core/` and `src-tauri/templates/`.
- Some identity metadata and a login script remain under `src-tauri/` because native code reads them. Do not delete a directory based on its historical name.
- Feature documentation: `gpui/README.md` and `gpui/docs/`. Older documents under `docs/` may describe the removed host; the current build and validation guides take precedence.

## Work

Read callers and tests before editing. Preserve user changes, public behavior, saved data, licenses and upstream attribution. Keep changes local; do not move shared modules just to rename directories. Preserve the WebView pages and the mature PDFium renderer.

Do not use or create shared build caches. In particular, do not run `scripts/gpui-build.mjs`, which mirrors the source into `C:\Users\huzhibin\.cache\lattice-gpui-build`. Before compiling, testing or building, check whether that cache is in use. If occupied, skip compile/test/build validation, continue other requested work, and if none remains, stop and wait for instructions. Never stop another process to free the cache. When the cache is not occupied, run Cargo directly with the repository-local `gpui/target`; do not create another target tree. `cargo fmt` and metadata queries do not compile and need no build cache.

Use `pnpm build:islands`, `pnpm build:native`, `pnpm check` and focused shared-module tests. Read `docs/agent-validation.md` before validation or recovering a failed check.

Run the locally built GPUI binary on a scratch copy of `gpui/fixtures/e2e`, with isolated home and state directories. Do not use a real manuscript as a test fixture or stop unrelated processes. Report actual command outcomes and remaining failures separately.

The translation catalogs also feed Rust UI strings. Do not run a clean extraction over TypeScript alone and discard native messages. Shared module generators are `scripts/gpui-host-errors.mjs`, `scripts/gpui-animated-icons.mjs`, `scripts/gpui-symbol-glyphs.mjs` and `design-system/code-themes.mjs`.
