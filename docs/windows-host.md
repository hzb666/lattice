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
The embedded Windows Agent is enabled for Windows x64 when the packaged MXC
runtime manifest and the startup capability probe pass. Other Windows
architectures and development sidecars remain unavailable.
One-click setup installs TeX Live for the current user
(no administrator prompt, cancellable) and the managed paper tools, and installs
a missing package with `tlmgr` or MiKTeX's package manager. The optional rtex
live preview is still macOS-only.

### Agent write boundary

Windows x64 uses [MXC ProcessContainer](https://github.com/microsoft/mxc).
Each project gets its own Synara process, home, and temporary workspace. The
process tree can read the original project. MXC denies writes to that project
and access to its `.chem/` cache and `.git` metadata. Shell output stays in
the isolated writable directories. Lattice never applies arbitrary shell
changes or Git checkpoint contents to the original project.

`replace_project_text` saves an exact replacement in an existing `.tex`, `.md`,
`.sty`, `.cls`, or `.txt` file through the Lattice file transaction. Lattice
keeps the before and after text in private turn history. Git checkpoints read
the original manuscript files from the read-only mount. Undo uses the host's
turn history and checks for concurrent changes. Bibliography edits go through
the existing citation tools. Both the scratch workspace and checkpoints skip
`.chem/`, ChemDraw and MestReNova source files. The original chemistry tools
retain ownership of their documents, exports, and caches.

Before a new provider turn, the embedded composer waits for Lattice to refresh
the manuscript-only scratch mirror and fails closed if the host cannot do so.
Steers during a live turn and in-turn comment reads do not refresh it, so a
live turn's scratch state is not replaced; a queued draft refreshes when it
becomes the next turn.

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

The packaged runtime must declare `windowsHostManuscript: 1`, which the
Windows preparation script writes after applying the pinned Synara source
patch. On startup, Lattice needs a bound project, compatible Git for Windows,
the verified executor, and a successful MXC capability probe. Failure leaves
the Agent unavailable.

The standalone MXC probe also checks read-only access to a fixture project
with an explicit deny for its `.chem` cache and write access to a separate
scratch directory. In that policy, reads of a manuscript succeed, direct
manuscript edits and new `.bib` files fail, and the cache cannot be read.

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

The Windows GPUI build and Clean entry points share a per-project file lock
with Agent manuscript saves. Lattice records the project's NTFS identity
before the first save. Subsequent builds, including those after a restart,
execute project TeX under MXC. Clean removes recognized auxiliary files
through the host.

The isolated build reads the original project and writes to a separate
temporary directory. The host validates the PDF and SyncTeX pair and
publishes them through the project transaction alongside any root `.aux`,
`.log`, `.bbl`, and `.blg` from that build, in the selected auxiliary layout.
The direct MXC probe passed with x64 TeX Live and
`latexmk -cd`, and SyncTeX resolved a source file in the original project.
The integrated GPUI build and clean flows still need runtime validation.

Release acceptance covers a real provider turn from `main.tex` save through
preview, diff, and Undo, rejected protected writes by subprocesses, citations,
concurrent edits, restart and multi-window isolation, and existing chemistry
exports and caches.

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
