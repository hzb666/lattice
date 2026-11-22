import { createHash } from "node:crypto";
import { readFileSync, realpathSync } from "node:fs";
import { dirname, isAbsolute, relative } from "node:path";

export const MXC_VERSION = "1.1.0";
const WIN_X64_EXECUTOR_SHA256 = "6d466c6b0109bab4bcace6738e11f886d801ea977346db49707488fcbd3afc8a";

export function verifyWindowsExecutor(path) {
  if (process.platform !== "win32" || process.arch !== "x64" || !isAbsolute(path)) {
    throw new Error("Expected an absolute path to the Windows x64 MXC executor");
  }
  const executable = realpathSync.native(path);
  const digest = createHash("sha256").update(readFileSync(executable)).digest("hex");
  if (digest !== WIN_X64_EXECUTOR_SHA256) {
    throw new Error(`Expected wxc-exec.exe from @microsoft/mxc-sdk ${MXC_VERSION} (Windows x64)`);
  }
  return executable;
}

function localPath(path) {
  const resolved = realpathSync.native(path);
  if (!/^[a-z]:\\/i.test(resolved)) throw new Error("MXC requires a local drive path");
  return resolved;
}

function overlaps(first, second) {
  return [relative(first, second), relative(second, first)].some(
    (path) => !isAbsolute(path) && path !== ".." && !path.startsWith("..\\"),
  );
}

export function workspacePolicy({ original, staging, tools, node, state }) {
  const readwritePaths = [localPath(staging), ...(state ? [localPath(state)] : [])];
  const project = localPath(original);
  const executable = localPath(node);
  const readonlyPaths = [project, localPath(tools), dirname(executable)];
  if (readonlyPaths.some((path) => readwritePaths.some((writable) => overlaps(path, writable)))) {
    throw new Error("Read-only paths and the writable staging directory must be disjoint");
  }

  const enumeratePaths = new Set();
  for (const path of [...readonlyPaths, ...readwritePaths]) {
    let current = path;
    while (dirname(current) !== current) {
      current = dirname(current);
      enumeratePaths.add(current);
    }
  }

  return {
    version: "1.0.0",
    containment: "processcontainer",
    process: { commandLine: `"${executable}" --version`, cwd: readwritePaths[0], timeout: 20_000 },
    filesystem: { readonlyPaths, readwritePaths },
    processContainer: { filesystem: { enumeratePaths: [...enumeratePaths] } },
    network: { egress: { default: "deny" }, ingress: { default: "deny", hostLoopback: "allow" } },
    ui: { disable: false },
  };
}
