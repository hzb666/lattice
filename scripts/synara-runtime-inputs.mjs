// Repository files hashed by prepare-synara-sidecar.mjs alongside the Synara
// source revision. External runtime caches should key off the same inputs.
export const SYNARA_RUNTIME_INPUTS = [
  "scripts/synara-runtime.json",
  "scripts/synara-runtime-inputs.mjs",
  "scripts/prepare-synara-sidecar.mjs",
  "scripts/synara-codex-host.mjs",
  "scripts/lib/mxc-executor.mjs",
  "scripts/lib/util.mjs",
  "scripts/lib/node-runtime.mjs",
  "scripts/lib/codesign.mjs",
  "scripts/lib/runtime-prune.mjs",
  "scripts/lib/synara-server-prune.mjs",
];

export const SYNARA_WINDOWS_SOURCE_PATCH = "scripts/patches/synara-windows-host.patch";

export const SYNARA_WINDOWS_RUNTIME_INPUTS = [
  "scripts/lib/synara-windows-source.mjs",
  SYNARA_WINDOWS_SOURCE_PATCH,
];
