# Contributing to the GPUI checkout

Start with `README.md`, `CLAUDE.md` and `docs/agent-validation.md`. Feature ownership is documented in `gpui/README.md` and `gpui/docs/`.

Keep changes focused and preserve project files, upstream licenses and attribution. Native code lives in `gpui/`, WebView modules in `src/` and the shared backend in `src-tauri/lattice-core/`; keep those dependency boundaries intact.

Run `pnpm install --frozen-lockfile`, then `pnpm build`. Use the shared Cargo wrapper for native compilation and focused tests for the changed behavior. Use scratch projects for app scenarios, and report failed or skipped checks accurately.

Changes to the Overleaf wire format, OT semantics, permissions or persisted sync state need a regression fixture and a rollback that preserves local work. Read `docs/overleaf-protocol.md` first.

Do not automatically clean translation messages by scanning only the retained TypeScript files: the catalogs are also build inputs of the native UI. Keep the GPUI catalog, animation, symbol and code-theme generators current when changing their sources.

## Upstream updates

This fork is `hzb666/lattice` (`origin`); `leo1oel/lattice` is `upstream`. Develop on `gpui`. Keep `main` for the upstream tree, and preserve shared source paths so patches remain easy to apply.

Fetch and review upstream changes before selecting commits:

```sh
git fetch upstream
git cherry -v gpui upstream/main
git show <upstream-commit>
git cherry-pick -x <upstream-commit>
```

`git cherry` marks an equivalent patch already present with `-`. Review each remaining `+` for dependencies and changes to removed files; it is not a list to apply blindly. Keep the original commit reference with `-x`, run the relevant checks, then push `gpui`. Whole-branch merges need a separate review because they can restore the removed application.

Reviewed through `032f99db969bbaac2ca4760cc3fda191f583d692` on 2026-11-18. Applied `7c0a855e`, `de5782e9`, `b8beee04`, `b3dd2c79` and `a5199493`. Deferred:

| Commit | Reason |
| --- | --- |
| `2917675e` | macOS sandbox and optional Synara runtime update; review when enabling that runtime. |
| `469461b2`, `9856aff1` | Changes to the removed Tauri window and project-tab host. |
| `7304f7b5` | Budget for the removed startup benchmark. |
| `145182ef` | Tauri release version bump; retained manifests remain at the fork's existing version. |
| `032f99db` | Plugin host phase 1 of 4; execution is currently macOS-only, so it does not provide Windows plugin support. |
