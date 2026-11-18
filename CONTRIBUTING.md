# Contributing to the GPUI checkout

Start with `README.md`, `CLAUDE.md` and `docs/agent-validation.md`. Feature ownership is documented in `gpui/README.md` and `gpui/docs/`.

Keep changes focused and preserve project files, upstream licenses and attribution. Native code lives in `gpui/`, WebView modules in `src/` and the shared backend in `src-tauri/lattice-core/`; keep those dependency boundaries intact.

Run `pnpm install --frozen-lockfile`, then `pnpm build`. Use the shared Cargo wrapper for native compilation and focused tests for the changed behavior. Use scratch projects for app scenarios, and report failed or skipped checks accurately.

Changes to the Overleaf wire format, OT semantics, permissions or persisted sync state need a regression fixture and a rollback that preserves local work. Read `docs/overleaf-protocol.md` first.

Do not automatically clean translation messages by scanning only the retained TypeScript files: the catalogs are also build inputs of the native UI. Keep the GPUI catalog, animation, symbol and code-theme generators current when changing their sources.
