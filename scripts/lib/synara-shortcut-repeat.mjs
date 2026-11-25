const SHORTCUT_PAYLOAD_ANCHOR = "altKey: event.altKey,";

export function patchSynaraShortcutRepeat(source) {
  if (/altKey:\s*event\.altKey,\s*repeat:\s*event\.repeat,/.test(source)) return source;

  const matches = source.split(SHORTCUT_PAYLOAD_ANCHOR).length - 1;
  if (matches !== 1) {
    throw new Error(
      `Expected one Synara host-shortcut payload anchor, found ${matches}. Re-check the repeat patch.`,
    );
  }

  const newline = source.includes("\r\n") ? "\r\n" : "\n";
  return source.replace(
    SHORTCUT_PAYLOAD_ANCHOR,
    `${SHORTCUT_PAYLOAD_ANCHOR}${newline}        repeat: event.repeat,`,
  );
}
