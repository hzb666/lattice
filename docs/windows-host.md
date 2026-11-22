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
The embedded Agent remains disabled until the Windows write guard is shipped
and verified. It must protect `.bib`, `.chem/`, ChemDraw and MestReNova files.
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

To preserve the macOS editing workflow on Windows, use a signed [file-system
minifilter](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/about-file-system-filter-drivers).
The GPUI installer should carry the driver package and request elevation for
driver setup. The editor and Agent should then run as the ordinary user.
GPUI currently has a Windows build and CI smoke run. The NSIS notes elsewhere
in this document describe the former Tauri host. The GPUI Agent setup targets
x64 Windows 11 24H2 and later. Its
[`package-installer.ps1`](../gpui/windows/package-installer.ps1) packages the
prebuilt editor and Synara resources with the signed driver INF, CAT and SYS.
It checks the staged Synara and Node versions and the runtime architecture
against the release configuration.
Release packaging requires
[driver signing](https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/driver-signing-offerings)
and a Microsoft-assigned [filter altitude](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/minifilter-altitude-request).
The first installer expects the previous release to be uninstalled before
installing a replacement.

The GPUI launcher checks the driver's protocol and verifies the project and
Synara state volumes before creating Synara suspended. It enrolls the process
before resuming it. Its Windows Job Object owns the process tree through
shutdown. `windows-agent-guard` is off in standard builds pending driver
validation. An opted-in build refuses to start the Agent if the driver cannot
verify coverage or enroll the process. The launcher opens each directory
without following a final reparse point. The filter checks mutation targets
before opening them, and refuses targets it cannot verify.
The GPUI panel, Synara sidecar and project review flow are shared with macOS.
The filter must allow ordinary project edits and the
trusted Lattice host's writes, while denying direct changes to protected
assets, including new files, replacements, deletion, renames and aliases.
The existing citation tools remain the supported Agent path to update `.bib`.

The Windows guard smoke test requires Node 24 and a local NTFS temporary
directory. The script in
[`scripts/check-windows-agent-guard.mjs`](../scripts/check-windows-agent-guard.mjs)
takes an absolute launcher path followed by its arguments.

```powershell
node scripts/check-windows-agent-guard.mjs (Resolve-Path gpui/target/release/lattice-gpui.exe).Path --lattice-guard-probe
```

The launcher receives
`-- <node.exe> <probe-script> --worker <scratch-project>` after those arguments,
starts that workload through the same protection as Synara and returns its
exit status. The probe checks ordinary edits and protected reads and writes
from the launched process and a child. It includes assets created by the host
after enrollment and checks that the host can still write. It also exercises
the pinned Synara runtime's SQLite configuration: `locking_mode=EXCLUSIVE`
before the first WAL access keeps the WAL index in memory, without a `-shm`
mapping. Validate startup and persistence with the bundled Synara runtime too.

Before enabling the Agent, close the remaining gaps in the prototype:

- Ensure that decisions about file identity and rename targets stay valid
  through the mutation, including concurrent link and namespace changes.
- Verify coverage for writes targeting unfiltered volumes, existing writable
  mappings, and operations with no safely attributable requestor. The local
  NTFS check and preflight name query do not prove cross-volume coverage.
- Review writes delegated to processes outside the enrolled tree. The Job
  Object manages lifetime and does not establish a file access boundary.

Run the probe against the signed driver on a disposable Windows installation.
Exercise the same chat, citation tools, Review, Undo, build and preview flow as
on macOS, and verify install, replacement, uninstall and restart behavior.

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
