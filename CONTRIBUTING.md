# Contributing to the GPUI checkout

Start with `README.md`, `CLAUDE.md` and `docs/agent-validation.md`. Feature ownership is documented in `gpui/README.md` and `gpui/docs/`.

Keep changes focused and preserve project files, upstream licenses and attribution. Native code lives in `gpui/`, WebView modules in `src/` and the shared backend in `src-tauri/lattice-core/`; keep those dependency boundaries intact.

Run `pnpm install --frozen-lockfile`, then `pnpm build`. Use the shared Cargo wrapper for native compilation and focused tests for the changed behavior. Use scratch projects for app scenarios, and report failed or skipped checks accurately.

Changes to the Overleaf wire format, OT semantics, permissions or persisted sync state need a regression fixture and a rollback that preserves local work. Read `docs/overleaf-protocol.md` first.

Do not automatically clean translation messages by scanning only the retained TypeScript files: the catalogs are also build inputs of the native UI. Keep the GPUI catalog, animation, symbol and code-theme generators current when changing their sources.

## Branches and upstream updates

This fork is `hzb666/lattice` (`origin`); `leo1oel/lattice` is `upstream`.
The native GPUI checkout is the product baseline:

- `main` tracks `origin/gpui` and contains the maintained GPUI baseline.
- `chemex` tracks `origin/chemex` and contains the chemistry work. Short-lived
  `feature/<name>` branches start here and return through a pull request.
- `tauri-main` tracks the old `origin/main` Tauri line for historical access; it
  is not a base for GPUI or Chemex work.

Do not merge `origin/main` into `main` or `chemex`. It is a separate product
line. Preserve shared source paths so upstream GPUI patches remain easy to
apply.

Fetch and review upstream changes before selecting commits:

```sh
git fetch upstream
git cherry -v main upstream/main
git show <upstream-commit>
git cherry-pick -x <upstream-commit>
```

`git cherry` marks an equivalent patch already present with `-`. Review each remaining `+` for dependencies and changes to removed files; it is not a list to apply blindly. Keep the original commit reference with `-x`, run the relevant checks, then push `main` (which tracks the remote GPUI branch). Whole-branch merges need a separate review because they can restore the removed application.

Apply accepted upstream commits to `main`, verify them, then merge `main` into
`chemex`. Do not cherry-pick upstream commits directly into `chemex` unless the
change is Chemex-specific and cannot belong in the shared baseline. The normal
flow is:

```sh
git switch main
git pull --ff-only
git fetch upstream
git cherry-pick -x <reviewed-upstream-commit>
# run focused checks, then push main
git push

git switch chemex
git pull --ff-only
git merge --no-ff main
# run Chemex checks, then push chemex
git push
```

Use `git merge --no-ff` for the long-lived `main` to `chemex` handoff so the
baseline synchronization remains visible. Use short-lived feature branches for
new work; do not rewrite already-pushed `main` or `chemex` history.

Reviewed through `032f99db969bbaac2ca4760cc3fda191f583d692` on 2026-11-18. Applied `7c0a855e`, `de5782e9`, `b8beee04`, `b3dd2c79` and `a5199493`. Deferred:

| Commit | Reason |
| --- | --- |
| `2917675e` | macOS sandbox and optional Synara runtime update; review when enabling that runtime. |
| `469461b2`, `9856aff1` | Changes to the removed Tauri window and project-tab host. |
| `7304f7b5` | Budget for the removed startup benchmark. |
| `145182ef` | Tauri release version bump; retained manifests remain at the fork's existing version. |
| `032f99db` | Plugin host phase 1 of 4; execution is currently macOS-only, so it does not provide Windows plugin support. |
