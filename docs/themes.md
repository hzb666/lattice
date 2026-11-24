# Themes

A theme recolors Lattice. It is data, not code: one `lattice-theme.json`
file that gives literal colors to the design system's base roles. The GPUI
app resolves themes with the shared Rust code in
[`src-tauri/lattice-theme`](../src-tauri/lattice-theme) and derives other
colors from the result. A theme cannot change sizes, spacing, fonts or
behavior, and it cannot run anything, so installing one asks for no
permission and works on every platform.

## The format

```json
{
  "$schema": "https://leo1oel.github.io/lattice-plugins/schema/theme-1.json",
  "schema": 1,
  "id": "dev.example.my-theme",
  "name": "My theme",
  "version": "1.0.0",
  "author": "Example",
  "license": "MIT",
  "description": "One sentence.",
  "variants": [
    {
      "id": "day",
      "name": "My theme Light",
      "appearance": "light",
      "colors": { "chrome": "#f2e9e1", "text": "#575279", "accent": "#907aa9", "line": "#5752791a" },
      "syntax": { "keyword": "#286983", "comment": "#9893a5" },
      "terminal": { "red": "#b4637a" }
    }
  ]
}
```

- **`id`** is reverse-domain and lower case. `app.lattice.*` belongs to the
  themes that ship with Lattice.
- **Variants:** one to eight, each for `light` or `dark`. Lattice uses the
  first variant for the appearance the window has. A theme with only a dark
  variant leaves the light appearance as Lattice's own.
- **Values are literal `#RRGGBB` or `#RRGGBBAA`.** No `var()`, `color-mix()`,
  names or functions: the GPUI app has no CSS engine, and literals keep the
  file safe to apply. Roles that color text must be opaque.
- **Every role is optional.** A role a theme leaves out keeps Lattice's own
  color for that appearance.
- Unknown top-level keys are ignored (a theme written for a newer Lattice
  still loads). An unknown role is an error. Files are at most 256 KiB.

The JSON Schema, generated from the role tables, is
[`design-system/themes/theme-1.schema.json`](../design-system/themes/theme-1.schema.json).

### Roles

| Section | Roles | Held to |
|---|---|---|
| `colors` surfaces | `window`, `chrome`, `panel`, `raised`, `input`, `editor`, `pdf-ground` | used as given |
| `colors` inks | `text`, `text-secondary` | 4.5:1 on every surface |
| `colors` lines | `line`, `line-strong` (usually translucent) | used as given |
| `colors` accent | `accent`, `focus`, `selection` | accent 4.6:1 on the chrome, panels and editor |
| `colors` status | `danger`, `success`, `warning`, `info`, `annotation` | 4.5:1 on the panel and lift |
| `syntax` | the 16 token roles of `design-system/code-themes.json` | 4.5:1 on the editor |
| `terminal` | `black` … `white`, `bright-black` … `bright-white` | used as given |

"Held to" means a color that does not read is moved, keeping its hue, to the
nearest one that does (`lattice_theme::color::fit_contrast`). The surfaces
are the theme's own, or Lattice's where it leaves one out.

Derived colors are not themeable: hover and pressed washes, the soft accent,
the titlebar veil, the status washes, the selection surface. GPUI derives them
from the base roles. That keeps interaction states consistent with the theme,
including states added later.

## Choosing a theme and an accent

- A theme replaces the tint: while one is in effect the app stands on
  Graphite and the theme sets the roles it names.
- **Accent:** `Theme default` (`accent: "theme"`) takes the
  theme's accent, or Graphite's when there is no theme or it has none. An
  accent the writer picks (a preset or a custom color) wins over the theme's,
  fitted to the theme's surfaces.
- A theme that colors the syntax wins over the Code theme setting while it is
  chosen.
- The choice is `appearance.theme` (the id, or `""`) in shared settings.

## Where themes live

`lattice-core::themes::ThemeStore` lists the bundled themes
(`design-system/themes/*.lattice-theme.json`, compiled in) and the installed
ones in `<app data>/themes/<id>.json`. Installing validates the file and keeps
a copy; the picked file can move or change afterwards. Nothing in a project
folder installs or selects a theme.

## Theme market

`lattice-core::registry` fetches the signed index and block list from the
Lattice registry. Each signature must verify against one of the pinned
minisign keys and bind the file name and serial. Both files must share a
serial; refreshes reject invalid signatures and rollback to an older list.
The last verified pair is cached in `<app data>/registry/cache.json`, verified
again on read, and served offline.

Theme downloads require HTTPS and must match the signed size, SHA-256, id and
version before install. `<app data>/registry/installed.json` records the
installed version for update offers. The signed block list prevents blocked
versions from installing. It also turns off a blocked registry-installed
theme without changing the saved choice; `warn` entries leave it applied and
show their reason. File-installed themes are not subject to the registry's
block list.

Settings › Appearance › Theme files › Browse themes searches the verified
listing and shows cached results while refreshing. Theme files are downloaded
only after the writer chooses Install or Update.

## How GPUI applies a theme

`Palette::with_theme` (`gpui/crates/lattice-ui/src/palette.rs`) lays the
resolved roles over Graphite and derives colors in Rust. The code palette and
terminal take the theme's syntax and ANSI colors. The active theme is also
passed to embedded WebView surfaces through `src/agent/agent-host-theme.ts`.

## Keeping colors consistent

- Accent and theme fitting run in `lattice_theme::color`. The TypeScript
  `fitContrast` used by code-theme export and syntax highlighting is checked
  against the Rust implementation by `src/settings/color-parity.test.ts` and
  `design-system/themes/color-vectors.json`.
- `gpui/crates/lattice-ui/src/theme_parity.rs` records the colors GPUI derives
  for bundled themes, appearances and accent choices in
  `design-system/themes/parity.json`.

For regeneration and validation commands, follow `docs/agent-validation.md`;
keep Cargo build output in the repository-local `gpui/target`.

## Bundled themes

Rosé Pine (Dawn and Main) and Nord (Snow Storm and Polar Night), both MIT,
mapped onto Lattice's roles. See THIRD_PARTY_NOTICES.md.
