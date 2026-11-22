import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { capture, sha256 } from "./util.mjs";

const source = "https://registry.npmjs.org/@microsoft/mxc-sdk/-/mxc-sdk-1.1.0.tgz";
const archiveSha256 = "22806bcc1ff61768c47c8463905129cd479a96bcfcd54c9acf93f316e4b82e19";
const licenseSha256 = "d9a1b1e30d633d5732ea18e3cba9538d293ebc53e1a9e4e96ab739e0c5c4f1cb";

export const MXC_EXECUTOR = Object.freeze({
  package: "@microsoft/mxc-sdk",
  version: "1.1.0",
  source,
  path: "bin/wxc-exec.exe",
  sha256: "6d466c6b0109bab4bcace6738e11f886d801ea977346db49707488fcbd3afc8a",
  license: "licenses/MXC-MIT.md",
});

const files = [
  ["bin/x64/wxc-exec.exe", MXC_EXECUTOR.path, MXC_EXECUTOR.sha256],
  ["LICENSE.md", MXC_EXECUTOR.license, licenseSha256],
];

export function mxcExecutorMatches(root, manifest) {
  return Object.entries(MXC_EXECUTOR).every(([key, value]) => manifest?.[key] === value) &&
    files.every(([, destination, digest]) => {
      const file = join(root, destination);
      return existsSync(file) && sha256(readFileSync(file)) === digest;
    });
}

export async function stageMxcExecutor(root, packageRoot) {
  let archive;
  if (!packageRoot) {
    const response = await fetch(source, { redirect: "error", signal: AbortSignal.timeout(120_000) });
    if (!response.ok) throw new Error(`Could not download MXC ${MXC_EXECUTOR.version}: HTTP ${response.status}`);
    archive = Buffer.from(await response.arrayBuffer());
    if (sha256(archive) !== archiveSha256) throw new Error("MXC package failed its SHA-256 check");
  }

  const verified = files.map(([member, destination, digest]) => {
    const bytes = packageRoot ? readFileSync(join(packageRoot, member)) :
      capture("tar", ["-xOf", "-", `package/${member}`], {
        input: archive,
        stdio: ["pipe", "pipe", "pipe"],
        encoding: "buffer",
        timeout: 30_000,
      });
    if (sha256(bytes) !== digest) throw new Error(`MXC ${member} failed its SHA-256 check`);
    return [join(root, destination), bytes];
  });
  for (const [file, bytes] of verified) {
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, bytes);
  }
}
