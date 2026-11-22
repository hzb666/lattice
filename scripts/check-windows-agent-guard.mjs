import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import {
  appendFileSync,
  copyFileSync,
  existsSync,
  linkSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(import.meta.url);
const extensions = ["bib", "CDX", "cdxml", "MnOvA", "MNJS", "mnl", "mnlt"];
const protectedFiles = [
  ...extensions.map((extension) => `assets/existing.${extension}`),
  "state/.CHEM/cache/preview.svg",
  "chemistry-only/.chem/database.sqlite",
];
const lateFiles = [
  ...extensions.map((extension) => `assets/host-created.${extension}`),
  "host-created/.chem/cache.txt",
];
const allProtectedFiles = [...protectedFiles, ...lateFiles];
const workerReady = "worker-ready.txt";
const hostReady = "host-ready.txt";
const launchTimeoutMs = 30_000;
const markerTimeoutMs = 10_000;

function denied(operation, action) {
  try {
    action();
  } catch (error) {
    assert.ok(["EACCES", "EPERM"].includes(error.code), `${operation}: ${error.message}`);
    return;
  }
  assert.fail(`Protected operation succeeded: ${operation}`);
}

function run(executable, args, cwd) {
  const result = spawnSync(executable, args, { cwd, windowsHide: true, timeout: launchTimeoutMs });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`Child exited with ${result.status ?? result.signal}\n${result.stderr}`);
  }
}

function requireScratch(root) {
  assert.ok(
    root && isAbsolute(root) && dirname(root) === realpathSync(tmpdir()) &&
      /^lattice-agent-guard-[a-z0-9]{6}$/i.test(basename(root)),
    "The guard workload requires its scratch fixture",
  );
  assert.equal(realpathSync(root), root, "Expected the resolved scratch directory");
}

async function waitForMarker(file, signal) {
  const deadline = performance.now() + markerTimeoutMs;
  while (!existsSync(file)) {
    assert.ok(performance.now() < deadline, `Timed out waiting for ${basename(file)}`);
    await delay(25, undefined, { signal });
  }
}

function editOrdinaryFiles(path, file) {
  appendFileSync(path(file), " edited");
  const edited = readFileSync(path(file), "utf8");
  assert.equal(edited, "manuscript edited", `append ${file}`);
  writeFileSync(path("new.tex"), `${edited} and saved`, { flag: "wx" });
  renameSync(path("new.tex"), path(file));
  assert.equal(readFileSync(path(file), "utf8"), "manuscript edited and saved", `save ${file}`);

  writeFileSync(path("new.tex"), "ordinary file", { flag: "wx" });
  renameSync(path("new.tex"), path("renamed.tex"));
  assert.equal(readFileSync(path("renamed.tex"), "utf8"), "ordinary file", "rename ordinary file");
  unlinkSync(path("renamed.tex"));
}

function checkExclusiveWal(file) {
  for (const phase of ["write", "reopen"]) {
    const database = new DatabaseSync(file);
    try {
      assert.equal(database.prepare("PRAGMA locking_mode = EXCLUSIVE").get().locking_mode, "exclusive");
      assert.equal(database.prepare("PRAGMA journal_mode = WAL").get().journal_mode, "wal");
      database.exec("BEGIN IMMEDIATE");
      if (phase === "write") {
        database.exec("CREATE TABLE smoke (value TEXT NOT NULL)");
        database.prepare("INSERT INTO smoke (value) VALUES (?)").run("persisted");
      }
      assert.equal(database.prepare("SELECT value FROM smoke").get().value, "persisted", `SQLite ${phase}`);
      assert.ok(existsSync(`${file}-wal`), `Missing SQLite WAL during ${phase}`);
      assert.ok(!existsSync(`${file}-shm`), `Unexpected SQLite -shm during ${phase}`);
      database.exec("COMMIT");
    } finally {
      database.close();
    }
  }
}

async function workload(root, child) {
  requireScratch(root);
  const path = (relative) => join(root, relative);
  if (!child) {
    writeFileSync(path(workerReady), "ready", { flag: "wx" });
    await waitForMarker(path(hostReady));
  }
  editOrdinaryFiles(path, child ? "child.tex" : "paper.tex");
  if (!child) checkExclusiveWal(path("state.sqlite"));
  for (const file of allProtectedFiles) {
    assert.equal(readFileSync(path(file), "utf8"), file, `read ${file}`);
  }
  for (const extension of extensions) {
    const file = `new.${extension}`;
    denied(`create ${file}`, () => writeFileSync(path(file), "new", { flag: "wx" }));
    denied(`rename into ${file}`, () => renameSync(path("replacement.tmp"), path(file)));
  }
  for (const file of allProtectedFiles) {
    denied(`append ${file}`, () => appendFileSync(path(file), "changed"));
    denied(`overwrite ${file}`, () => writeFileSync(path(file), "changed", { flag: "r+" }));
    denied(`replace ${file}`, () => copyFileSync(path("replacement.tmp"), path(file)));
    denied(`rename over ${file}`, () => renameSync(path("replacement.tmp"), path(file)));
    denied(`rename ${file}`, () => renameSync(path(file), path("moved.txt")));
    denied(`delete ${file}`, () => unlinkSync(path(file)));
  }
  denied("create .chem directory", () => mkdirSync(path(".chem")));
  denied("create inside .chem", () => writeFileSync(path("state/.CHEM/new.txt"), "new"));
  denied("rename parent of .chem", () => renameSync(path("chemistry-only"), path("moved-dir")));
  denied("write through existing hard link", () => appendFileSync(path("linked.txt"), "changed"));
  denied("write through directory junction", () => appendFileSync(path("asset-junction/existing.bib"), "changed"));
  denied("write through .chem junction", () => appendFileSync(path("chem-junction/cache.txt"), "changed"));
  denied("create a writable alias", () => linkSync(path("assets/existing.bib"), path("new-link.txt")));
  denied("write alternate data stream", () => writeFileSync(`${path("assets/existing.bib")}:extra`, "changed"));
  if (!child) run(process.execPath, [script, "--child", root], root);
}

async function publishLateFiles(root, signal) {
  await waitForMarker(join(root, workerReady), signal);
  for (const file of lateFiles) {
    const target = join(root, file);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, file, { flag: "wx" });
  }
  symlinkSync(join(root, "host-created/.chem"), join(root, "chem-junction"), "junction");
  writeFileSync(join(root, hostReady), "ready", { flag: "wx" });
}

async function runWorker(launcher, launcherArgs, root) {
  const probe = spawn(launcher, [...launcherArgs, "--", process.execPath, script, "--worker", root], {
    cwd: root,
    windowsHide: true,
    stdio: "inherit",
    timeout: launchTimeoutMs,
  });
  const stopWaiting = new AbortController();
  const exit = once(probe, "exit").finally(() => stopWaiting.abort());
  const [result, publication] = await Promise.allSettled([exit, publishLateFiles(root, stopWaiting.signal)]);
  if (result.status === "rejected") throw result.reason;
  if (publication.status === "rejected" && publication.reason.code !== "ABORT_ERR") throw publication.reason;
  const [status, signal] = result.value;
  assert.equal(status, 0, `Guard launcher exited with ${status ?? signal}`);
  if (publication.status === "rejected") throw publication.reason;
}

async function check(launcher, launcherArgs) {
  assert.ok(launcher, "Usage: node scripts/check-windows-agent-guard.mjs <launcher> [args...]");
  const root = mkdtempSync(join(realpathSync(tmpdir()), "lattice-agent-guard-"));
  requireScratch(root);
  const path = (relative) => join(root, relative);
  try {
    for (const file of protectedFiles) {
      mkdirSync(dirname(path(file)), { recursive: true });
      writeFileSync(path(file), file);
    }
    writeFileSync(path("replacement.tmp"), "replacement");
    writeFileSync(path("paper.tex"), "manuscript");
    writeFileSync(path("child.tex"), "manuscript");
    linkSync(path("assets/existing.bib"), path("linked.txt"));
    symlinkSync(path("assets"), path("asset-junction"), "junction");
    await runWorker(launcher, launcherArgs, root);
    for (const file of ["paper.tex", "child.tex"]) {
      assert.equal(readFileSync(path(file), "utf8"), "manuscript edited and saved", `edit ${file}`);
    }
    for (const file of allProtectedFiles) {
      assert.equal(readFileSync(path(file), "utf8"), file, `preserve ${file}`);
    }
    assert.equal(readFileSync(path("replacement.tmp"), "utf8"), "replacement", "preserve replacement source");
    for (const file of [
      ".chem",
      "moved.txt",
      "moved-dir",
      "new-link.txt",
      "new.tex",
      "renamed.tex",
      "state/.CHEM/new.txt",
      "assets/existing.bib:extra",
      ...extensions.map((extension) => `new.${extension}`),
    ]) {
      assert.ok(!existsSync(path(file)), `unexpected ${file}`);
    }
    writeFileSync(path("assets/existing.bib"), "trusted host edit");
    assert.equal(readFileSync(path("assets/existing.bib"), "utf8"), "trusted host edit");
    console.log("Windows Agent write protection passed");
  } finally {
    requireScratch(root);
    // Unlink the junctions before removing the scratch directory.
    for (const junction of ["asset-junction", "chem-junction"]) {
      if (existsSync(path(junction))) unlinkSync(path(junction));
    }
    rmSync(root, { recursive: true, force: true });
  }
}

assert.equal(process.platform, "win32", "This check requires Windows");
const [mode, ...args] = process.argv.slice(2);
if (mode === "--worker" || mode === "--child") {
  await workload(args[0], mode === "--child");
} else {
  await check(mode, args);
}
