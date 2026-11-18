# Running the GPUI application

Build with `pnpm build` from the repository root. Run `gpui/target/release/lattice-gpui.exe <project>` on Windows, or the executable without `.exe` on macOS. PDFium, the built island pages and a TeX distribution with `latexmk` must be available; see `README.md`.

Use a scratch copy of `gpui/fixtures/e2e` for automated checks. Isolate the home, state and Windows application-data directories, then run:

```text
lattice-gpui <scratch-project> --scenario e2e --only smoke --out <result.json>
```

Keep logs and the result, apply a bounded timeout, and verify both scenario steps and save-on-quit. The native process exiting successfully does not mean every assertion passed. Read `docs/agent-validation.md` for the evidence rules.

The old Tauri mock-backend page and cross-host visual comparison harness have been removed. Native scenario and screenshot support remains in `gpui/crates/lattice-gpui/src/scenario/`; its available scenarios and platform restrictions are documented in `gpui/README.md`.
