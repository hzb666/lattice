# Experimental Windows host

Windows support currently covers project file operations, PDF reads, process
supervision, a ConPTY terminal and discovery of installed TeX distributions.
It requires Windows 10 version 1809 or later (Windows 11 recommended).

This is an experimental target. Releases whose Windows installer checks pass
also provide a per-user x64 NSIS installer, clearly marked experimental.
Download the `windows-x64-experimental-setup.exe` and its matching `.sha256`
from [GitHub Releases](https://github.com/leo1oel/lattice/releases). Release
downloads need no GitHub account and have no Actions artifact expiry.
The installer is **unsigned**: SmartScreen may show an unknown-publisher warning.
Check the source and checksum before deciding whether to run it; do not disable
Windows security protections. See the matching `.README.txt` asset for setup.

There is no Windows auto-update: download each new version manually.
Authenticode signing and Windows updater support are separate future work.
The existing macOS/Linux release jobs are unchanged.
The embedded Windows Agent stays disabled pending native verification.
One-click setup installs TeX Live for the current user
(no administrator prompt, cancellable) and the managed paper tools, and installs
a missing package with `tlmgr` or MiKTeX's package manager. The optional rtex
live preview is still macOS-only.

### Agent write boundary

The Windows design uses [MXC ProcessContainer](https://github.com/microsoft/mxc)
for process isolation. Synara and its subprocesses work in a writable copy,
without access to the original project. Lattice applies permitted changes to the
original. The writeback policy protects `.bib`, `.chem/`, ChemDraw and
MestReNova assets, including files created after the Agent starts. That keeps
protected changes off the original, but does not stop direct edits inside the
disposable copy. Citation tools retain their separate, validated bibliography
write path.

The local probe uses Node 24, an NTFS checkout and `package/bin/x64/wxc-exec.exe`
from `@microsoft/mxc-sdk@1.1.0`. It verifies the executor hash before launch.
Check the [MXC Windows build requirements](https://github.com/microsoft/mxc/blob/v1.1.0/docs/backends/process-container/os-version-support.md)
for loopback support. The probe needs ordinary user permissions.

```powershell
New-Item -ItemType Directory -Force .tmp/mxc-probe | Out-Null
npm pack @microsoft/mxc-sdk@1.1.0 --pack-destination .tmp/mxc-probe --ignore-scripts
tar -xf .tmp/mxc-probe/microsoft-mxc-sdk-1.1.0.tgz -C .tmp/mxc-probe package/bin/x64/wxc-exec.exe
node scripts/check-windows-agent-sandbox.mjs .tmp/mxc-probe/package/bin/x64/wxc-exec.exe
```

The probe uses a temporary fixture under `.tmp/`. It checks loopback health,
parent and child write restrictions, and fixture writeback. Batches with a
protected path or concurrent writer edit are rejected in full.
Pass an already prepared `src-tauri/synara-runtime` as the second argument
to check startup, loopback health, and shutdown with the pinned Synara build.

`pnpm prepare:synara` stages the pinned executor and its MIT license on
Windows x64. The bundled Synara health and shutdown probe has passed on
Windows 11 25H2 x64. The Rust launcher additionally checks Windows Job
membership and ownership of the loopback listener. Its native test
`bundled_windows_sidecar_starts_and_stops_inside_mxc` requires the staged
runtime and a Windows build with MXC loopback support.

The MXC probe ran the local Git for Windows `2.53.0` executable read-only; its
attempted installation write was blocked. Provider networking is only
partially probed: npm returned HTTP 200 and Anthropic returned HTTP 403.
OpenAI timed out on this machine. No authenticated provider turn has been
verified.

The Windows GPUI Agent remains disabled. The implementation now creates a
separate Synara home and sidecar for each project, denies the original project,
and grants write access only to that project's staged root rather than its
shared parent. The local probe also confirmed that MXC cannot deny one file
under a writable project root while preserving ordinary writes: a denied
descendant makes the writable policy too broad or too restrictive depending on
the path combination. The host writeback policy still rejects protected paths
before touching the original, but this does not match macOS direct-write
protection. The project runtime and multi-window routing are not yet verified
in the GPUI application.

The implementation keeps the original project read-only to the sidecar and
derives permitted writeback paths from completed Git checkpoints on the active
thread; history replay and title changes supply no writeback paths. Before a
transaction starts, the host compares staged bytes with the selected Git blob
from the completed checkpoint or the turn-start checkpoint for Undo. It
handles loose objects and packed refs. Divergent bytes, protected assets,
missing objects, and Undo results that differ from the turn-start snapshot are
preserved in staging for recovery. Packed-only Git objects fail closed; pack
file reading is not implemented. The Node writeback helper is only for the
probe fixture.

An MXC smoke test with TeX Live 2026 x64 produced a PDF and working forward
and reverse SyncTeX with both `pdflatex` and `latexmk`. A separate figure
probe resolved an input under a read-only project path containing spaces and
wrote its outputs under a temporary directory. The installed TeX Live 2022
x86 could run on the host but failed to start under MXC with Windows error
623. TeX Live 2026 required `ui.disable: false` to start in this probe.

The Windows Agent figure preview uses the pinned MXC executor with read-only
project and TeX Live directories, a writable temporary directory, and network
ingress and egress denied. The figure probe also passed with container UI
isolation and clipboard and input injection blocked. This bridge still
requires GPUI runtime validation.

The Windows GPUI build and clean entry points share a per-project file lock
with Agent writeback. Before publishing the first changes, the host records
the project's NTFS identity in its private state. Future builds of that
project, including watcher builds and builds after restarting Lattice, use
MXC. Clean removes known hidden auxiliary files through the host.

The isolated build reads the original project and writes to a separate
temporary directory. The host validates the PDF and SyncTeX pair and
publishes them through the project transaction. Auxiliaries for these builds
use the hidden layout. The direct MXC probe passed with x64 TeX Live and
`latexmk -cd`, and SyncTeX resolved a source file in the original project.
The integrated GPUI build and clean flows still need runtime validation.

Normal and ChemEx project builds use the MXC TeX runner and host-side ChemEx
preparation. The separate marked-PDF Review and export paths still use their
existing runner. Windows Agent UI and checkpoint code remain dormant behind
the startup gate. Acceptance must cover authenticated provider turns, citation
tools with protected-file access denied, Review, Undo conflicts, rename
writeback, restart recovery, per-project multi-window use, and behavior when
MXC is unavailable. There is no UI flow yet for recovering an unpublished
staging copy after restarting Lattice.

The built-in Agent remains disabled in the GPUI Windows application, and this
checkout currently has no GPUI Windows installer or release artifact. The
existing experimental Tauri/NSIS installer instructions below do not package
the GPUI application and must not be used to infer its release readiness.

## TeX distributions

TeX Live and MiKTeX both work. MiKTeX's `latexmk` and `texcount` are Perl
scripts, and MiKTeX does not ship Perl: install one such as
[Strawberry Perl](https://strawberryperl.com/) or builds fail. MiKTeX also asks
before installing each missing package, and that dialog holds the build until
Lattice stops it after five minutes. In MiKTeX Console, set
**Settings → Always install missing packages on-the-fly**.

## Running from source

`pnpm tauri dev` needs the pinned Synara checkout described in
`CONTRIBUTING.md`. The first run stages Synara, which can take longer than the
180 seconds Tauri waits for the dev server, so stage it once on its own first:

```powershell
pnpm prepare:runtime:dev
pnpm tauri dev
```

## Installer builds and checks

See [the Windows installer implementation](windows-installer.md) for
triggers, isolated install/uninstall checks, public release assets and limits.
PR/manual builds provide a checked Actions ZIP (GitHub login required, 14-day
retention); only a successful upstream release can publish permanent assets.

To build locally after installing the prerequisites below:

```powershell
pnpm install --frozen-lockfile
pnpm tauri build --bundles nsis --config src-tauri/tauri.windows.conf.json
```

The build stages a SHA-256-verified standalone Node with its redistribution
license and the frozen Open Slide runtime. It does not need a Synara checkout
and does not bundle its unavailable Agent server, Pandoc or uv. Interface fonts
are open-licensed. No private fonts, signing secrets or product-license keys
are loaded by the Windows workflow; tldraw uses its unlicensed mode without a key.

## Host checks

Install the [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/),
the repository's pinned Rust toolchain, Node and pnpm. From the repository root:

```powershell
pnpm install --frozen-lockfile
New-Item -ItemType Directory -Force src-tauri/synara-runtime | Out-Null
Set-Content src-tauri/synara-runtime/placeholder.txt 'Compilation checks only'
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

The stub is sufficient for compilation and tests; it does not supply the bundled
Agent or presentation runtime. Link tests need Developer Mode or permission to
create Windows symbolic links. The Windows CI job runs these host checks without
packaging or publishing a binary. Its native tests exercise Unicode paths,
reparse-point refusal, atomic writes, file identities, process Job Objects and
ConPTY execution, resizing and window ownership.
