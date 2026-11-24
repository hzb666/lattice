# Contributing to the GPUI checkout

Start with `README.md`, `CLAUDE.md` and `docs/agent-validation.md`. Feature ownership is documented in `gpui/README.md` and `gpui/docs/`.

Keep changes focused and preserve project files, upstream licenses and attribution. Native code lives in `gpui/`, WebView modules in `src/` and the shared backend in `src-tauri/lattice-core/`; keep those dependency boundaries intact.

Run `pnpm install --frozen-lockfile`, then `pnpm build`. Use the shared Cargo wrapper for native compilation and focused tests for the changed behavior. Use scratch projects for app scenarios, and report failed or skipped checks accurately.

Changes to the Overleaf wire format, OT semantics, permissions or persisted sync state need a regression fixture and a rollback that preserves local work. Read `docs/overleaf-protocol.md` first.

Do not automatically clean translation messages by scanning only the retained TypeScript files: the catalogs are also build inputs of the native UI. Keep the GPUI catalog, animation, symbol and code-theme generators current when changing their sources.

## Branches and upstream updates

This fork is `hzb666/lattice` (`origin`); `leo1oel/lattice` is `upstream`.

- `chemex` tracks `origin/chemex` and is the active development branch for GPUI
  and Chemex. Start short-lived `feature/<name>` branches from `chemex` and
  target it in pull requests.
- `main` tracks `origin/main` and preserves the fork's GPUI baseline.
- `tauri-main` tracks the retired Tauri app for historical access.

Review `upstream/main` commit by commit and cherry-pick accepted changes directly
onto local `chemex`. Keep the retired Tauri host and its release pipeline out of
the GPUI development line. Preserve shared source paths so upstream GPUI patches
remain easy to apply.

Fetch and review upstream changes before selecting commits:

```sh
git switch chemex
git fetch upstream main
git cherry -v chemex upstream/main
git show <upstream-commit>
git cherry-pick -x <upstream-commit>
```

`git cherry` marks an equivalent patch already present with `-`. Review each
remaining `+` for dependencies and changes to removed files. Keep the original
commit reference with `-x` and run the relevant checks on `chemex`. Do not
rewrite already-pushed `main` or `chemex` history.

Use the [upstream sync log](docs/upstream-sync.md) for the last reviewed commit,
local backports, and pending changes. Update it after each review.
