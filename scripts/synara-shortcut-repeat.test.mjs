import { describe, expect, it } from "vitest";
import { patchSynaraShortcutRepeat } from "./lib/synara-shortcut-repeat.mjs";

describe("Synara host shortcut repeat patch", () => {
  it("forwards the KeyboardEvent repeat flag and is idempotent", () => {
    const source = [
      "window.parent.postMessage({",
      "        altKey: event.altKey,",
      "      },",
    ].join("\n");

    const patched = patchSynaraShortcutRepeat(source);

    expect(patched).toContain("altKey: event.altKey,\n        repeat: event.repeat,");
    expect(patchSynaraShortcutRepeat(patched)).toBe(patched);
  });

  it("preserves CRLF and rejects a drifted or ambiguous source", () => {
    const source = "        altKey: event.altKey,\r\n";
    expect(patchSynaraShortcutRepeat(source)).toBe(
      "        altKey: event.altKey,\r\n        repeat: event.repeat,\r\n",
    );
    expect(() => patchSynaraShortcutRepeat("altKey: press.altKey,")).toThrow(
      "Expected one Synara host-shortcut payload anchor, found 0",
    );
    expect(() => patchSynaraShortcutRepeat(`${source}altKey: event.altKey,`)).toThrow(
      "Expected one Synara host-shortcut payload anchor, found 2",
    );
  });
});
