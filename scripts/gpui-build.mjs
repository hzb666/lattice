#!/usr/bin/env node
/**
 * `node scripts/gpui-build.mjs`: builds the GPUI app (by default the release
 * build with `offscreen-shots`, which `gpui/script/e2e` runs) in this
 * checkout's gpui/target. Cargo's incremental artifacts stay with this
 * checkout instead of being rebuilt in a separate cache tree.
 *
 * Each run, under the machine-wide GPUI lock (scripts/lib/gpui-lock.mjs):
 *
 *   1. fetches PDFium into this checkout (`gpui/script/fetch-pdfium`), which
 *      the binary finds next to the GPUI workspace;
 *   2. runs `cargo <args>` in gpui/, with CARGO_BUILD_JOBS=4 unless set;
 *      a `cargo test` fails as soon as one test runs past the per-test limit
 *      (300 s, `LATTICE_GPUI_TEST_TIMEOUT_SECONDS`; scripts/lib/test-timeout.mjs)
 *      rather than holding the lock while it loops;
 * Send compiling Cargo commands through here to reuse gpui/target. A
 * --manifest-path is resolved against the invoking directory and must remain
 * inside this checkout.
 *
 * Usage:
 *   node scripts/gpui-build.mjs                  release build, offscreen-shots
 *   node scripts/gpui-build.mjs -- test --release --locked --workspace \
 *     --features lattice-gpui/offscreen-shots -- --include-ignored
 *                                                any other cargo command there
 *   node scripts/gpui-build.mjs -- clippy --all-targets -- -D warnings
 */
import { spawn, spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { acquireGpuiLock, releaseGpuiLock } from "./lib/gpui-lock.mjs";
import { localManifest } from "./lib/cargo-manifest.mjs";
import { testTimeoutSeconds, watchTestOutput } from "./lib/test-timeout.mjs";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DEFAULT_ARGS = ["build", "--release", "--locked", "--features", "lattice-gpui/offscreen-shots"];

function seconds(since) {
  return `${((Date.now() - since) / 1000).toFixed(0)} s`;
}

/**
 * Runs cargo without blocking the event loop, so a SIGTERM or Ctrl-C reaches
 * the lock module's handler at once, and stops cargo on the way out rather
 * than leaving it building after the lock is released.
 *
 * A `cargo test` runs in a process group of its own with its stdout read
 * line by line (scripts/lib/test-timeout.mjs): a test that runs past the
 * per-test limit stops the whole group, test binary included, and fails the
 * run naming it, instead of holding the build lock until someone notices.
 */
function runCargo(args, options) {
  const testing = args[0] === "test" || args[0] === "t";
  return new Promise((resolve, reject) => {
    const child = spawn("cargo", args, { ...options, stdio: ["inherit", testing ? "pipe" : "inherit", "inherit"], detached: testing });
    const signal = (name) => {
      try {
        // The group, so the test binary cargo started goes too.
        process.kill(testing ? -child.pid : child.pid, name);
      } catch {
        // Already gone.
      }
    };
    const stop = () => child.exitCode === null && signal("SIGTERM");
    process.on("exit", stop);
    let timedOut = null;
    let watch = null;
    if (testing) {
      const limitSeconds = testTimeoutSeconds();
      watch = watchTestOutput({
        limitSeconds,
        onTimeout(name) {
          timedOut = name;
          console.error(`\n${new Date().toISOString()} gpui-build: test ${name} ran past the ${limitSeconds} s per-test limit (LATTICE_GPUI_TEST_TIMEOUT_SECONDS); stopping cargo test`);
          signal("SIGTERM");
          setTimeout(() => child.exitCode === null && signal("SIGKILL"), 5_000).unref();
        },
      });
      let pending = "";
      child.stdout.on("data", (chunk) => {
        process.stdout.write(chunk);
        pending += chunk.toString("utf8");
        const lines = pending.split("\n");
        pending = lines.pop();
        for (const line of lines) watch.line(line);
      });
    }
    child.on("error", reject);
    child.on("close", (code, signalName) => {
      process.off("exit", stop);
      watch?.stop();
      if (timedOut) console.error(`gpui-build: failed: test ${timedOut} timed out`);
      resolve(timedOut ? 1 : (code ?? (signalName ? 1 : 0)));
    });
  });
}

async function main() {
  const separator = process.argv.indexOf("--");
  const givenArgs = separator >= 0 ? process.argv.slice(separator + 1) : DEFAULT_ARGS;
  if (separator < 0 && process.argv.length > 2) throw new Error(`unknown option ${process.argv[2]}; pass cargo arguments after --`);
  if (givenArgs.length === 0) throw new Error("no cargo arguments after --");

  const cargoArgs = localManifest(givenArgs, repo);
  const gpui = path.join(repo, "gpui");
  const target = path.join(gpui, "target");

  await acquireGpuiLock(repo);
  try {
    let started = Date.now();
    const windows = process.platform === "win32";
    const fetchCommand = windows ? "pwsh.exe" : path.join(gpui, "script", "fetch-pdfium");
    const fetchArgs = windows ? ["-NoProfile", "-NonInteractive", "-File", path.join(gpui, "script", "fetch-pdfium.ps1")] : [];
    const fetched = spawnSync(fetchCommand, fetchArgs, { cwd: gpui, stdio: "inherit" });
    if (fetched.status !== 0) throw new Error("gpui/script/fetch-pdfium failed");
    const env = { ...process.env, CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? "4" };
    env.CARGO_TARGET_DIR = target;
    delete env.CARGO_BUILD_TARGET_DIR;
    started = Date.now();
    console.log(`gpui-build: cargo ${cargoArgs.join(" ")} (in ${gpui}, target ${target})`);
    const status = await runCargo(cargoArgs, { cwd: gpui, env });
    if (status !== 0) {
      process.exitCode = status;
      console.error(`gpui-build: cargo failed after ${seconds(started)}`);
      return;
    }
    console.log(`gpui-build: cargo finished in ${seconds(started)}`);
  } finally {
    releaseGpuiLock();
  }
}

await main().catch((error) => {
  console.error(`gpui-build: ${error.message}`);
  process.exit(1);
});
