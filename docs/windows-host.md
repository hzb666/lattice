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
The embedded Agent remains disabled because Windows has no hard write guard
for `.bib`, `.chem/`, ChemDraw or MestReNova files. AppContainer and ACLs can
limit access to named paths, but cannot enforce this extension rule for files
the Agent creates later. A complete Windows implementation needs a host write
broker or a signed file-system minifilter; Lattice does not ship either yet.
One-click setup installs TeX Live for the current user
(no administrator prompt, cancellable) and the managed paper tools, and installs
a missing package with `tlmgr` or MiKTeX's package manager. The optional rtex
live preview is still macOS-only.

### Agent write boundary

Windows AppContainer is useful for reducing a process to an explicit identity
and a set of ACL-granted directories. Windows 11 also exposes the experimental
`Experimental_CreateProcessInSandbox` API with Bound File System path grants,
but it is still path-based and not an extension filter. ACLs
are attached to file and directory objects and are inherited by newly created
objects; they cannot say "allow every project write except a future `.bib` or
`.cdxml` file". Renames also do not acquire a new default ACL merely because
the destination directory has one. See Microsoft's [AppContainer launch
guide](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer),
[sandbox process API](https://learn.microsoft.com/en-us/windows/win32/secauthz/createprocessinsandbox)
and [file security and access rights](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights).

The two viable designs are a host write broker, where the Agent can only stage
validated changes for the parent to apply, or a signed kernel file-system
minifilter that rejects create/write/rename operations for the protected
extensions and `.chem` directories. A minifilter can prevent I/O in the file
system stack, as described in Microsoft's [minifilter overview](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/about-file-system-filter-drivers),
but it adds a signed driver and installer lifecycle. Lattice therefore keeps
Windows Agent fail-closed until one of those designs is implemented and tested.

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
