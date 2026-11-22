import { lstatSync, readFileSync, readdirSync, realpathSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { join, posix } from "node:path";

const privateDirectories = new Set([".chem", ".git", ".research", ".omp"]);
const protectedExtensions = new Set([".bib", ".cdx", ".cdxml", ".mnova", ".mnjs", ".mnl", ".mnlt"]);

function safeSegment(name) {
  return name !== "." && name !== ".." && !/[<>:"/\\|?*\u0000-\u001f]|[. ]$/u.test(name)
    && !/^(?:con|prn|aux|nul|conin\$|conout\$|com[1-9¹²³]|lpt[1-9¹²³]) *(?:\.|$)/iu.test(name);
}

function protectedPath(path) {
  const parts = path.toLowerCase().split("/");
  const name = parts.at(-1);
  const extensionStart = name.lastIndexOf(".");
  return parts.some((part) => privateDirectories.has(part))
    || (extensionStart >= 0 && protectedExtensions.has(name.slice(extensionStart)));
}

export function readWorkspace(root) {
  const resolved = realpathSync.native(root);
  const files = new Map();
  const names = new Set();
  function scan(relative) {
    for (const name of readdirSync(join(resolved, relative))) {
      const path = posix.join(relative, name);
      if (!safeSegment(name) || names.has(path.toLowerCase())) {
        throw new Error(`Ambiguous workspace path: ${path}`);
      }
      names.add(path.toLowerCase());
      const absolute = join(resolved, path);
      const info = lstatSync(absolute);
      if (info.isDirectory() && !info.isSymbolicLink() && realpathSync.native(absolute) === absolute) {
        scan(path);
      } else if (info.isFile() && info.nlink === 1 && realpathSync.native(absolute) === absolute) {
        files.set(path, readFileSync(absolute));
      } else {
        throw new Error(`Unsupported workspace entry: ${path}`);
      }
    }
  }
  scan("");
  return files;
}

function sameContents(first, second) {
  return first === undefined ? second === undefined : second !== undefined && first.equals(second);
}

export function planWriteback(baseline, staging, original) {
  const changes = [];
  const refused = [];
  const conflicts = [];
  const originalNames = new Map([...original.keys()].map((path) => [path.toLowerCase(), path]));
  for (const path of new Set([...baseline.keys(), ...staging.keys()])) {
    const before = baseline.get(path);
    const after = staging.get(path);
    if (sameContents(before, after)) continue;
    if (protectedPath(path)) {
      refused.push(path);
    } else if (
      !sameContents(before, original.get(path))
      || (originalNames.has(path.toLowerCase()) && originalNames.get(path.toLowerCase()) !== path)
    ) {
      conflicts.push(path);
    } else {
      changes.push({ path, before, after });
    }
  }
  return { changes, refused, conflicts };
}

export function applyFixtureWriteback(root, { changes, refused, conflicts }) {
  if (refused.length > 0) throw new Error("The batch changes protected paths");
  if (conflicts.length > 0) throw new Error("The batch conflicts with writer edits");
  const resolved = realpathSync.native(root);
  const current = readWorkspace(resolved);
  for (const { path, before } of changes) {
    const parts = path.split("/");
    if (parts.some((part) => !part || !safeSegment(part)) || protectedPath(path)) {
      throw new Error(`Refusing writeback path: ${path}`);
    }
    if (!sameContents(before, current.get(path))) throw new Error(`File changed before writeback: ${path}`);
  }
  for (const { path, before, after } of changes) {
    const destination = join(resolved, path);
    if (after === undefined) {
      unlinkSync(destination);
    } else if (before === undefined) {
      writeFileSync(destination, after, { flag: "wx" });
    } else {
      const temporary = join(resolved, posix.dirname(path), `.lattice-${randomUUID()}.tmp`);
      writeFileSync(temporary, after, { flag: "wx" });
      try {
        renameSync(temporary, destination);
      } catch (error) {
        unlinkSync(temporary);
        throw error;
      }
    }
  }
}
