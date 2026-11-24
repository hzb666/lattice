import { mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { run } from "./util.mjs";

export function prepareWindowsSynaraSource(checkout, revision, patch) {
  const parent = realpathSync(tmpdir());
  const root = mkdtempSync(join(parent, "lattice-synara-"));
  process.once("exit", () => removeTemporaryCheckout(root, parent));

  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^GIT_/i.test(key)));
  env.GIT_CONFIG_NOSYSTEM = "1";
  env.GIT_CONFIG_GLOBAL = join(root, ".git", "lattice-empty-config");
  env.GIT_ATTR_NOSYSTEM = "1";
  run("git", [
    "clone", "--quiet", "--shared", "--no-checkout", "--template=",
    "--config", "core.autocrlf=false", "--config", "core.eol=lf", "--config", "core.longpaths=true",
    "--config", `core.attributesFile=${join(root, ".git", "lattice-empty-attributes")}`,
    "--config", `core.hooksPath=${join(root, ".git", "lattice-disabled-hooks")}`,
    "--", checkout, root,
  ], { env });
  run("git", ["checkout", "--quiet", "--detach", revision], { cwd: root, env });
  try {
    run("git", ["apply", "--index", "--whitespace=nowarn", "-"], {
      cwd: root,
      env,
      input: patch,
      stdio: ["pipe", "inherit", "inherit"],
    });
  } catch (cause) {
    throw new Error(`Could not apply the Windows Synara source patch to ${revision}.`, { cause });
  }
  return { root, env };
}

function removeTemporaryCheckout(root, parent) {
  try {
    const resolved = realpathSync(root);
    if (resolved !== root || dirname(resolved) !== parent) {
      throw new Error(`Temporary Synara checkout moved outside its expected path: ${root}`);
    }
    rmSync(resolved, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
  } catch (error) {
    if (error.code !== "ENOENT") {
      console.error(`Could not remove the temporary Synara checkout: ${error.message}`);
      process.exitCode ||= 1;
    }
  }
}
