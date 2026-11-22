import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";
import { MXC_EXECUTOR, mxcExecutorMatches } from "./mxc-executor.mjs";
import { capture, isMain, projectRoot, readJson, writeJson } from "./util.mjs";
import { verifyWindowsExecutor, workspacePolicy } from "./windows-agent-sandbox.mjs";

async function waitForSynara(statePath, signal) {
  const deadline = performance.now() + 25_000;
  while (performance.now() < deadline) {
    signal.throwIfAborted();
    if (existsSync(statePath)) {
      const { port, origin } = readJson(statePath);
      assert.ok(Number.isInteger(port) && port > 0 && port <= 65_535);
      assert.equal(origin, `http://127.0.0.1:${port}`);
      const response = await fetch(`${origin}/health`, {
        signal: AbortSignal.any([signal, AbortSignal.timeout(500)]),
        redirect: "error",
      }).catch(() => null);
      if (response?.ok && (await response.json()).startupReady === true) return origin;
    }
    await delay(100, undefined, { signal });
  }
  throw new Error("Synara did not report readiness inside MXC");
}

function checkProviderNetwork(executor, node, { control, config, scratch }) {
  const worker = join(scratch, "provider-network.mjs");
  writeFileSync(worker, `
const endpoints = [
  ["npm", "https://registry.npmjs.org/-/ping"],
  ["OpenAI", "https://api.openai.com/v1/models"],
  ["Anthropic", "https://api.anthropic.com/v1/models", { "anthropic-version": "2023-06-01" }],
];
const results = await Promise.all(endpoints.map(async ([name, url, headers]) => {
  try {
    const response = await fetch(url, { headers, redirect: "error", signal: AbortSignal.timeout(8_000) });
    await response.body?.cancel();
    return { name, status: response.status };
  } catch (error) {
    return { name, error: error.cause?.code ?? error.name };
  }
}));
console.log(JSON.stringify(results));
`, { flag: "wx" });
  const results = JSON.parse(capture(executor, [config, "--", node, worker], { cwd: control, timeout: 12_000 }));
  for (const { name, status, error } of results) {
    console.log(`MXC HTTPS ${name}: ${status ? `HTTP ${status}` : error}`);
  }
  const [npm, ...providers] = results;
  assert.equal(npm.status, 200, "Public HTTPS control failed");
  assert.ok(providers.some(({ status }) => Number.isInteger(status)), "No provider returned an HTTPS response");
}

export async function checkSynara(executor, runtime, { root, original, staging, probeProvider = false }) {
  const expected = readJson(join(projectRoot, "scripts/synara-runtime.json"));
  const manifest = readJson(join(runtime, "manifest.json"));
  assert.equal(manifest.synaraRevision, expected.revision);
  assert.equal(manifest.nodeVersion, expected.nodeVersion);
  assert.equal(manifest.target, "x86_64-pc-windows-msvc");
  assert.equal(manifest.nodeRuntime, "standalone");
  assert.match(manifest.migrationRuntimeSourceDigest ?? "", /^[a-f0-9]{64}$/, "Invalid migration source digest");

  const node = join(runtime, "bin/node.exe");
  const server = join(runtime, "server");
  const entry = join(server, "dist/index.mjs");
  assert.ok(existsSync(entry), "Synara service entry point is missing");
  assert.equal(capture(node, ["--version"], { timeout: 5_000 }).trim(), `v${expected.nodeVersion}`);
  const state = join(root, "synara");
  const scratch = join(state, "temp");
  const control = join(root, "synara-control");
  mkdirSync(scratch, { recursive: true });
  mkdirSync(control);
  const userdata = join(state, "userdata");
  mkdirSync(userdata);
  writeFileSync(join(userdata, "environment-id"), `${randomUUID()}\n`, { flag: "wx" });
  console.log("Preseeded Synara's fixture environment-id (MXC denies first-run hard-link creation)");
  const token = randomUUID();
  const shutdown = randomUUID();
  const systemRoot = process.env.SystemRoot;
  assert.ok(systemRoot, "Windows SystemRoot is required for the MXC probe");
  const policy = workspacePolicy({ original, staging, tools: runtime, node, state });
  const [originalPath, runtimePath] = policy.filesystem.readonlyPaths;
  policy.filesystem.readonlyPaths = [runtimePath];
  policy.filesystem.deniedPaths = [originalPath, control];
  const programFiles = process.env.ProgramW6432 ?? process.env.ProgramFiles;
  const installedGit = programFiles && join(programFiles, "Git");
  const gitRoot = installedGit && existsSync(installedGit) ? realpathSync.native(installedGit) : null;
  if (gitRoot) policy.filesystem.readonlyPaths.push(gitRoot);
  policy.processContainer.filesystem.enumeratePaths =
    policy.processContainer.filesystem.enumeratePaths.filter((path) => path !== runtimePath);
  if (probeProvider) policy.network.egress.default = "allow";
  policy.process = {
    ...policy.process,
    cwd: state,
    timeout: 40_000,
    inheritDefaultEnv: false,
    env: [
      `SYSTEMROOT=${systemRoot}`, `WINDIR=${systemRoot}`,
      `PATH=${join(runtime, "bin")};${gitRoot ? `${join(gitRoot, "cmd")};${join(gitRoot, "mingw64", "bin")};` : ""}${join(systemRoot, "System32")}`,
      `HOME=${state}`, `USERPROFILE=${state}`, `LOCALAPPDATA=${state}`, `APPDATA=${state}`,
      `TEMP=${scratch}`, `TMP=${scratch}`, `TMPDIR=${scratch}`, `SYNARA_HOME=${state}`,
      `SYNARA_AUTH_TOKEN=${token}`, `SYNARA_DESKTOP_SHUTDOWN_TOKEN=${shutdown}`,
      `SYNARA_MIGRATION_RUNTIME_SOURCE_DIGEST=${manifest.migrationRuntimeSourceDigest}`,
      "NODE_ENV=production", "SYNARA_MODE=desktop", "SYNARA_HOST=127.0.0.1",
      "SYNARA_NO_BROWSER=true", "SYNARA_TELEMETRY_ENABLED=false", "AGENT_HOST_PROFILE=lattice",
    ],
  };
  const config = join(control, "synara-policy.json");
  const gitProbe = join(scratch, "git-readonly.mjs");
  if (gitRoot) {
    writeFileSync(gitProbe, `
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
const root = ${JSON.stringify(gitRoot)};
const result = spawnSync(join(root, "cmd", "git.exe"), ["--version"], { encoding: "utf8", windowsHide: true });
assert.equal(result.status, 0, result.stderr || result.error?.message);
assert.match(result.stdout, /^git version /);
assert.throws(() => writeFileSync(join(root, "mxc-probe-write"), "blocked"),
  (error) => ["EACCES", "EPERM"].includes(error.code));
console.log(result.stdout.trim() + "; installation write blocked");
`);
  }
  const prepareTemp = join(scratch, "prepare-temp.mjs");
  writeFileSync(prepareTemp, `
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, relative, sep } from "node:path";
const temporary = tmpdir();
const fromHome = relative(process.env.SYNARA_HOME, temporary);
assert.ok(fromHome && !isAbsolute(fromHome) && fromHome !== ".." && !fromHome.startsWith(".." + sep));
mkdirSync(temporary, { recursive: true });
`, { flag: "wx" });
  writeJson(config, policy);
  try {
    const child = spawn(executor, [config, "--", node, "--import", pathToFileURL(prepareTemp).href, entry, "--dynamic-port"], {
      cwd: control,
      windowsHide: true,
      timeout: 45_000,
      stdio: "inherit",
    });
    const lifetime = new AbortController();
    const exit = once(child, "exit").finally(() => lifetime.abort());
    const ready = async () => {
      const origin = await waitForSynara(join(userdata, "server-runtime.json"), lifetime.signal);
      const database = join(userdata, "state.sqlite");
      assert.ok(statSync(database).size > 0, "Synara did not initialize its state database");
      assert.ok(!existsSync(`${database}-shm`), "Synara created a shared SQLite WAL index");
      const response = await fetch(`${origin}/api/desktop/shutdown`, {
        method: "POST",
        headers: { Authorization: `Bearer ${shutdown}` },
        redirect: "error",
        signal: AbortSignal.timeout(2_000),
      });
      assert.equal(response.status, 202);
    };
    const [exited, checked] = await Promise.allSettled([exit, ready()]);
    if (exited.status === "rejected") throw exited.reason;
    if (checked.status === "rejected" && checked.reason.name !== "AbortError") throw checked.reason;
    assert.deepEqual(exited.value, [0, null], "Synara exited unsuccessfully");
    if (checked.status === "rejected") throw checked.reason;
    console.log(`Synara startup and shutdown passed under MXC: ${root}`);
    if (gitRoot) {
      console.log(capture(executor, [config, "--", node, gitProbe], {
        cwd: control,
        timeout: 15_000,
      }).trim());
    }

    if (probeProvider) checkProviderNetwork(executor, node, { control, config, scratch });
  } finally {
    rmSync(config, { force: true });
  }
}

if (isMain(import.meta.url)) {
  const [runtimePath = join(projectRoot, "src-tauri/synara-runtime"), ...extra] = process.argv.slice(2);
  assert.equal(extra.length, 0, "Usage: node scripts/lib/mxc-synara-probe.mjs [synara-runtime]");
  const runtime = resolve(runtimePath);
  const manifest = readJson(join(runtime, "manifest.json"));
  assert.ok(mxcExecutorMatches(runtime, manifest.mxcExecutor), "Packaged MXC executor or license failed verification");
  const executor = verifyWindowsExecutor(join(runtime, MXC_EXECUTOR.path));
  const temporary = join(projectRoot, ".tmp");
  mkdirSync(temporary, { recursive: true });
  const root = mkdtempSync(join(temporary, "mxc-synara-probe-"));
  const original = join(root, "original");
  const staging = join(root, "staging");
  mkdirSync(original);
  mkdirSync(staging);
  console.log(`Synara MXC probe fixture: ${root}`);
  await checkSynara(executor, runtime, { root, original, staging, probeProvider: true });
}
