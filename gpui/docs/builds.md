# Builds

One of the features [the GPUI app](../README.md) has so far.

Builds with lattice-core's latexmk build on one click (or ⌘S / Ctrl+S, which saves and builds, as in the Tauri app; ⇧-click rebuilds clean), once when a project is entered (whatever Settings › Editor & builds says), and after each save (the Tauri app's default automatic builds), and reloads the PDF in place.

Projects using `\usepackage{chemex}` can include a source drawing directly with `\includegraphics{figure.cdxml}`. Keep the main document and drawings inside the project folder. On Windows, Lattice evaluates the manuscript's LaTeX counters, writes numbered CDXML copies, and reuses or creates their ChemDraw SVG/PDF assets in `.chem/cache/chemdraw/`. MestReNova PDFs are kept in `.chem/cache/mestrenova/`. The default `compound` counter advances at each drawing's first field in source order; prose `\chemex{key}` references read the numbering pass's map, so inserting a drawing renumbers later drawings and prose together. Custom counters retain LaTeX's native order and reset behavior. ChemDraw exports the SVG, and Lattice converts it to vector PDF using its built-in `svg2pdf` library. The final paper goes through Lattice's normal incremental latexmk build. Unchanged Chemex inputs reuse their registrations and assets. When one drawing changes, only drawings whose numbered contents changed need new exports.

The Chemistry panel's **All** view offers the source CDXML and the rendered, numbered drawing; it can read the SVG already exported for the project's main paper when the source drawing matches. Without a matching build asset, it renders a numbered source copy. Group buttons show each key and number, while group SVGs omit those labels from the image. The preview can be zoomed and scrolled. On Windows, the visible panel also prewarms the selected drawing's group SVGs on disk, one at a time, after loading the preview. A build, a new selection, or hiding the panel pauses the remaining work. Recent rasterized previews are retained in memory.

A write the window's open editors do not own (the Agent, a terminal, another editor, an Overleaf pull) to a build input builds once the writes pause for 1.5 s, and so does an outside edit of an open file; files only a build writes (its aux, log and synctex files, the root document's PDF, bbl, -blx.bib and achemso's acs-<name>.bib, `_minted`/`svg-inkscape`, the atomic-write temporary, `.git`, and whatever the last pass's recorder named) start no build.
After a build that stalled, automatic builds wait until an input changes; Build still runs on request.
A successful build of an Overleaf-linked project becomes an "Auto-saved version" git commit, at most every 2 minutes.
The Build button's hover card lists the passes latexmk ran; a build can be stopped, a switch to another project asks first while one runs, and the build files cleaned; a quiet (live-preview) build's success shows the chip's check without the flourish.
To open the docked build output, run **Show or hide the build log** from the command palette. **Messages** lists errors and warnings; **Log** shows the raw compiler output.
The build's diagnostics are the Tauri app's card over the PDF (or docked under it, a choice kept for every project and imported from the Tauri app's): TeX's messages in plain words with the failing line quoted, Lattice's own build advice in the interface language (copies and logs stay in English), a typo's one-click fix, a missing package's install, the whole raw log, and errors that jump to their line (F8 / ⇧F8 step through them).
Each open editor underlines the whole line of each of the build's diagnostics while its text is the text the build compiled, so the marks go on an edit and come back on its undo.
A failed build's card offers Fix all, which hands the build log and its errors to the Agent through the compile-repair task relay, under the permission mode the writer chose in the Agent, and each explained error offers Ask the agent for that one error.
While the repair runs, the card shows its progress, every editor of the project is read-only, and Cancel repair stops it (Unlock anyway frees the editors once a cancel has not stopped it within 10 s).
When the Agent is not available (no provider connected), the card says why instead of starting a repair; for an Overleaf collaborator without write access, Fix all and Ask the agent are disabled.
Once a repair has run, the card's footer says whether the build after it still fails and offers View repair, which opens the Agent thread that did it.

On Windows, **TeX Diagnose** lists MestReNova as an optional tool. It reads the executable's version without starting the app or checking its license. It uses the same lookup as spectrum export: `CHEMEX_MESTRENOVA`, `PATH`, an already running instance, then Windows installation records.

## In the e2e scenario

The `watched_builds` module (`scenario/e2e/watched_builds.rs`): a write to a file no editor has open building once the writes pause, a burst of such writes building once after the last of them, an outside edit of an open file waiting for the pause too, a file only a build writes starting no build (nor, open in an editor, making the PDF older when it reloads), and the card's docked or floating choice kept in the settings.
The `build_marks` module (`scenario/e2e/build_marks.rs`): the broken build's error underlined in the editor whose text it compiled, gone on an edit and back on its undo.
The `compile_repair` module (`scenario/e2e/compile_repair.rs`): Fix all on the broken build locking the editors while the repair starts, and, with no Agent service staged in a scratch run, the card saying why with the editors writable again and Fix all offered once more.
