import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import {
  existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, renameSync, symlinkSync, unlinkSync, writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { projectRoot, writeJson } from "./lib/util.mjs";
import { verifyWindowsExecutor, workspacePolicy } from "./lib/windows-agent-sandbox.mjs";
import { applyFixtureWriteback, planWriteback, readWorkspace } from "./lib/agent-sandbox-writeback.mjs";
import { checkSynara } from "./lib/mxc-synara-probe.mjs";

const assets = [
  "paper.tex", "concurrent.tex", "remove.tex", "refs.BIB", ".chem/cache.txt", "late.bib",
  "figure.cdx", "figure.cdxml", "sample.mnova", "script.mnjs", "report.mnl", "report.mnlt",
];

async function awaitHost(original) {
  const { createServer } = require("node:http");
  const { once } = require("node:events");
  const fs = require("node:fs");
  const { join } = require("node:path");
  const { setTimeout: delay } = require("node:timers/promises");
  const server = createServer((_request, response) => response.end("sandbox health"));
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  try {
    fs.writeFileSync("ready.tmp", String(server.address().port), { flag: "wx" });
    fs.renameSync("ready.tmp", "ready.signal");
    const deadline = Date.now() + 8_000;
    while (!fs.existsSync(join(original, "late.bib")) && Date.now() < deadline) await delay(25);
    fs.unlinkSync("ready.signal");
  } finally {
    server.close();
  }
}

async function sandboxWorker() {
  const assert = require("node:assert/strict");
  const { execFileSync } = require("node:child_process");
  const fs = require("node:fs");
  const { join } = require("node:path");
  const [original, mode, ...files] = process.argv.slice(2);
  if (mode === "readonly-project") {
    const denied = { code: /^(EACCES|EPERM|ENOENT)$/ };
    assert.equal(fs.readFileSync(join(original, "paper.tex"), "utf8"), files[0]);
    assert.throws(() => fs.appendFileSync(join(original, "paper.tex"), " edited"), denied);
    assert.throws(() => fs.readFileSync(join(original, ".chem/cache.txt")), denied);
    assert.throws(() => fs.writeFileSync(join(original, "future.bib"), "edited"), denied);
    fs.writeFileSync("generated.txt", "scratch write", { flag: "wx" });
    return;
  }
  if (mode === "parent") await awaitHost(original);
  for (const file of files) {
    const path = join(original, file);
    assert.equal(fs.readFileSync(path, "utf8"), file);
    assert.throws(() => fs.appendFileSync(path, " edited"), { code: /^(EACCES|EPERM)$/ });
  }
  assert.throws(() => fs.writeFileSync(join(original, "new.bib"), "test"), { code: /^(EACCES|EPERM)$/ });
  if (mode === "parent") {
    assert.throws(() => fs.renameSync(join(original, "refs.BIB"), join(original, "moved.tex")), {
      code: /^(EACCES|EPERM)$/,
    });
    for (const file of files.filter((file) => file !== "late.bib" && file !== "remove.tex")) {
      fs.writeFileSync(join(process.cwd(), file), "staged edit");
    }
    fs.unlinkSync(join(process.cwd(), "remove.tex"));
    fs.writeFileSync(join(process.cwd(), "late.bib"), "staged edit");
    fs.renameSync("move.tex", "new.bib");
    execFileSync(process.execPath, [__filename, original, "child", ...files], {
      timeout: 10_000,
      windowsHide: true,
    });
  } else {
    fs.writeFileSync(join(process.cwd(), "child.tex"), "child");
  }
}

async function publishAfterLaunch(staging, original, signal) {
  const ready = join(staging, "ready.signal");
  const deadline = performance.now() + 5_000;
  while (!existsSync(ready)) {
    assert.ok(performance.now() < deadline, "MXC workload did not report ready");
    await delay(25, undefined, { signal });
  }
  const port = Number(readFileSync(ready, "utf8"));
  assert.ok(Number.isInteger(port) && port > 0 && port <= 65_535, "Invalid probe port");
  const health = await fetch(`http://127.0.0.1:${port}/health`, {
    redirect: "error",
    signal: AbortSignal.any([signal, AbortSignal.timeout(2_000)]),
  });
  assert.equal(await health.text(), "sandbox health");
  const pending = join(original, "late.tmp");
  writeFileSync(pending, "late.bib", { flag: "wx" });
  renameSync(pending, join(original, "late.bib"));
}

const [executorPath, runtimePath, ...extra] = process.argv.slice(2);
assert.ok(executorPath && !extra.length,
  "Usage: node scripts/check-windows-agent-sandbox.mjs <wxc-exec.exe> [synara-runtime]");
const executor = verifyWindowsExecutor(resolve(executorPath));
const temporary = join(projectRoot, ".tmp");
mkdirSync(temporary, { recursive: true });
const root = mkdtempSync(join(temporary, "agent-sandbox-"));
const original = join(root, "original");
const staging = join(root, "staging");
const tools = join(root, "tools");
mkdirSync(staging);
mkdirSync(tools);

function seed(directory) {
  for (const file of assets.filter((file) => file !== "late.bib")) {
    const path = join(directory, file);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, file, { flag: "wx" });
  }
}
seed(original);
seed(staging);
for (const folder of [original, staging]) writeFileSync(join(folder, "move.tex"), "rename source");
const baseline = readWorkspace(original);

const policy = join(root, "policy.json");
const workload = join(tools, "probe.cjs");
writeJson(policy, workspacePolicy({ original, staging, tools, node: process.execPath }));
const worker = `${awaitHost.toString()}\n(${sandboxWorker.toString()})().catch((error) => { console.error(error); process.exitCode = 1; });\n`;
writeFileSync(workload, worker, { flag: "wx" });

const options = { stdio: "inherit", timeout: 30_000, windowsHide: true };
execFileSync(executor, ["--probe", policy], options);
const child = spawn(executor, [policy, "--", process.execPath, workload, original, "parent", ...assets], options);
const publication = new AbortController();
const [ran, published] = await Promise.allSettled([
  once(child, "exit").finally(() => publication.abort()),
  publishAfterLaunch(staging, original, publication.signal),
]);
if (ran.status === "rejected") throw ran.reason;
assert.deepEqual(ran.value, [0, null], "MXC workload exited unsuccessfully");
if (published.status === "rejected") throw published.reason;

for (const file of assets) assert.equal(readFileSync(join(original, file), "utf8"), file);
writeFileSync(join(original, "concurrent.tex"), "writer edit");
const blocked = planWriteback(baseline, readWorkspace(staging), readWorkspace(original));
assert.deepEqual(blocked.refused.toSorted(), [...assets.filter((file) => !file.endsWith(".tex")), "new.bib"].sort());
assert.throws(() => applyFixtureWriteback(original, blocked), /protected paths/);
assert.equal(readFileSync(join(original, "move.tex"), "utf8"), "rename source");
for (const file of blocked.refused) {
  const before = baseline.get(file);
  if (before === undefined) unlinkSync(join(staging, file));
  else writeFileSync(join(staging, file), before);
}
writeFileSync(join(staging, "move.tex"), baseline.get("move.tex"), { flag: "wx" });
const concurrent = planWriteback(baseline, readWorkspace(staging), readWorkspace(original));
assert.deepEqual(concurrent.conflicts, ["concurrent.tex"]);
assert.throws(() => applyFixtureWriteback(original, concurrent), /writer edits/);
assert.equal(readFileSync(join(original, "paper.tex"), "utf8"), "paper.tex");
writeFileSync(join(staging, "concurrent.tex"), baseline.get("concurrent.tex"));
const plan = planWriteback(baseline, readWorkspace(staging), readWorkspace(original));
assert.deepEqual(plan.changes.map(({ path }) => path).sort(), ["child.tex", "paper.tex", "remove.tex"]);
writeFileSync(join(original, "paper.tex"), "newer writer edit");
assert.throws(() => applyFixtureWriteback(original, plan), /changed before writeback/);
assert.equal(existsSync(join(original, "child.tex")), false);
writeFileSync(join(original, "paper.tex"), baseline.get("paper.tex"));
applyFixtureWriteback(original, plan);
const result = readWorkspace(original);
assert.equal(result.get("paper.tex").toString(), "staged edit");
assert.equal(result.get("child.tex").toString(), "child");
assert.equal(result.get("concurrent.tex").toString(), "writer edit");
assert.equal(result.has("remove.tex"), false);
for (const file of blocked.refused) assert.equal(result.get(file)?.toString(), file === "new.bib" ? undefined : file);
linkSync(join(original, "paper.tex"), join(original, "linked.tex"));
assert.throws(() => readWorkspace(original), /Unsupported workspace entry/);
unlinkSync(join(original, "linked.tex"));
symlinkSync(staging, join(original, "junction"), "junction");
assert.throws(() => readWorkspace(original), /Unsupported workspace entry/);
unlinkSync(join(original, "junction"));
console.log(`MXC isolation and fixture writeback passed: ${root}`);

const scratch = join(root, "readonly-project-scratch");
mkdirSync(scratch);
const readonlyProject = workspacePolicy({ original, staging: scratch, tools, node: process.execPath });
readonlyProject.filesystem.deniedPaths = [join(readonlyProject.filesystem.readonlyPaths[0], ".chem")];
const readonlyConfig = join(root, "readonly-project-policy.json");
writeJson(readonlyConfig, readonlyProject);
execFileSync(executor, [readonlyConfig, "--", process.execPath, workload, original, "readonly-project",
  result.get("paper.tex").toString()], options);
assert.equal(readFileSync(join(scratch, "generated.txt"), "utf8"), "scratch write");
console.log("MXC read-only project, denied chemistry cache, and writable scratch passed");

if (runtimePath) await checkSynara(executor, resolve(runtimePath), { root, original, staging });
