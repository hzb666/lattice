import { describe, expect, it } from "vitest";
import path from "node:path";
import { localManifest } from "./cargo-manifest.mjs";

describe("localManifest", () => {
  const repo = path.resolve("workspace");
  const cwd = path.join(repo, "gpui");

  it("resolves both manifest forms within the checkout", () => {
    expect(localManifest(["build", "--manifest-path", "Cargo.toml"], repo, cwd)).toEqual([
      "build",
      "--manifest-path",
      path.join(cwd, "Cargo.toml"),
    ]);
    expect(localManifest(["check", "--manifest-path=../src-tauri/lattice-core/Cargo.toml"], repo, cwd)).toEqual([
      "check",
      `--manifest-path=${path.join(repo, "src-tauri", "lattice-core", "Cargo.toml")}`,
    ]);
  });

  it("leaves arguments after the test separator unchanged", () => {
    expect(localManifest(["test", "--", "--manifest-path", "fixture"], repo, cwd)).toEqual([
      "test",
      "--",
      "--manifest-path",
      "fixture",
    ]);
  });

  it("rejects manifests outside the checkout", () => {
    expect(() => localManifest(["build", "--manifest-path", "../../outside/Cargo.toml"], repo, cwd))
      .toThrow("is outside");
  });
});
