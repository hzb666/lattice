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

Compiling Cargo commands run through `node scripts/gpui-build.mjs -- <cargo arguments>` from the repository root. Builds use the repository-local `gpui/target` under the build lock. `cargo fmt` and metadata queries do not need the wrapper.

Use `pnpm build:islands`, `pnpm build:native`, `pnpm check` and focused shared-module tests. Read `docs/agent-validation.md` before validation or recovering a failed check.

Run the locally built GPUI binary on a scratch copy of `gpui/fixtures/e2e`, with isolated home and state directories. Do not use a real manuscript as a test fixture or stop unrelated processes. Report actual command outcomes and remaining failures separately.

The translation catalogs also feed Rust UI strings. Do not run a clean extraction over TypeScript alone and discard native messages. Shared module generators are `scripts/gpui-host-errors.mjs`, `scripts/gpui-animated-icons.mjs`, `scripts/gpui-symbol-glyphs.mjs` and `design-system/code-themes.mjs`.
