# Boards, spreadsheets and decks

One of the features [the GPUI app](../README.md) has so far.

Opens `.excalidraw` files in the native GPUI whiteboard; opening a legacy `.tldr` converts it beside the original, which remains unchanged. Spreadsheets (`.lattice-sheet`) and Open Slide decks (`slides/<deck>/index.tsx`) remain WebView islands. Structured editors use the panel's buffer and save flow, merge external edits, and flush pending work before closing. Sheets export through the save panel; decks use the staged Open Slide runtime and expose Agent actions.
The deck's selected slide and element are the Agent's presentation context, a deck with an inspector edit not yet committed is held from Overleaf until it lands, and a deck on a linked project whose Overleaf role cannot write is shown read-only.
When another project's window holds the Open Slide runtime, Switch to that window brings it forward.
