# GPUI validation

This checkout contains the GPUI application and its shared dependencies. Validate the current source; an older executable or a successful WebView build does not prove the native build still works.

## Commands

From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm check
pnpm vitest run <changed-test-file>
pnpm build:islands
node scripts/gpui-build.mjs -- build --release --locked
node scripts/gpui-build.mjs -- test --release --locked --workspace
cargo fmt --manifest-path gpui/Cargo.toml --all -- --check
```

Only run checks relevant to the change. Send compiling Cargo commands through `scripts/gpui-build.mjs` to reuse `gpui/target`; it serializes GPUI builds with the machine-wide lock and fetches PDFium into `gpui/vendor`. Keep the shared backend at its current path and preserve Cargo lockfiles.

Generators have `--check`: `scripts/gpui-host-errors.mjs`, `scripts/gpui-animated-icons.mjs`, `scripts/gpui-symbol-glyphs.mjs` and `design-system/code-themes.mjs`. Their TypeScript, CSS, translation and JSON inputs remain part of GPUI even when native code is their only consumer.

## Runtime checks

Run only the locally built GPUI executable. Copy `gpui/fixtures/e2e` to a scratch project; use isolated `HOME`, `LATTICE_GPUI_STATE_DIR` and, on Windows, application-data folders. Put the TeX binaries on `PATH`. Point `PDFIUM_LIBRARY_PATH` at the downloaded library directory and `LATTICE_GPUI_ISLANDS_DIR` at the built pages when needed.

```text
lattice-gpui <scratch-project> --scenario e2e --only smoke --out <result.json>
```

On Windows the executable ends in `.exe`. The scenario posts events to its own windows. Bound the run, retain stdout/stderr and inspect the result's `passed` field and individual steps; a process exit of zero alone is insufficient. Also verify that `after_quit.file` contains `after_quit.text` after exit. Never overwrite a manuscript or stop an unrelated app.

## Evidence and recovery

Keep task logs, deletion manifests and checkpoints under ignored `.tmp/<task>/`. Record commands, exit codes, revision, local modifications, owned process IDs and the next action. Read the first error in a failed log, then rerun the narrowest check that tests the cause.

Separate typechecking, resource generation, native compilation, runtime behavior and visual fidelity. Report pre-existing and new failures honestly. Do not claim full regression coverage from a build or a small scenario.
