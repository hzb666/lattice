# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Existing repository stack: Vite, React, TypeScript, and shared dependencies. The landing page can use a self-contained Vite entry under `landing/`.

## Users

Researchers writing scientific manuscripts with LaTeX and managing figures and source documents.

## Product Purpose

Molume is the product name specified in the landing-page brief. The underlying repository documents a GPUI desktop writing workspace preview that brings scientific project files, LaTeX editing, PDF builds and preview, and ChemDraw figures into one workflow.

## Positioning

The project workflow keeps LaTeX source, compiled-paper preview, and editable ChemDraw source drawings connected, including Chemex numbering and vector export into a paper.

## Operating Context

Researchers work with local project folders, LaTeX manuscripts, PDFs, bibliography files, and editable scientific drawings. The documented GPUI app is a preview, not a confirmed public release.

## Capabilities and Constraints

- The GPUI app edits LaTeX and project files, builds with `latexmk`, previews PDFs, and supports SyncTeX navigation between source and PDF.
- Chemex integrates ChemDraw CDXML drawings with LaTeX; on Windows, the documented build uses ChemDraw exports and the normal LaTeX build flow.
- The app supports local projects. Do not claim offline guarantees, encryption, or a downloadable public release without evidence.
- No confirmed public download, pricing, customer testimonials, or institutional endorsements are present in the inspected repository.
- The landing-page brief names the site/product Molume; repository application documentation currently names the desktop app Lattice GPUI.

## Brand Commitments

The user-provided brief sets the Molume name and the line “Quiet by design. Powerful by nature.” It specifies warm minimalism, scientific editorial typography, full-bleed muted color sections, honest product claims, and respect for researchers' files and attention.

## Evidence on Hand

- Product feature documentation: `gpui/README.md` and `gpui/docs/`.
- Chemex integration documentation and examples: `src-tauri/chemex/README.md` and `src-tauri/chemex/examples/`.
- No landing-page screenshots, approved public product screenshots, or release download URL were found under `landing/`.

## Product Principles

- Keep scientific source drawings editable and authoritative.
- Connect writing, build output, and figures without inventing capabilities.
- Keep product claims and release status verifiable.
- Preserve researchers' focus and local project workflow.

## Accessibility & Inclusion

The landing page should support keyboard navigation, clear focus states, responsive layouts, semantic content, and reduced motion.
