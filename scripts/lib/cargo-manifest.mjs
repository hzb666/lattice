import path from "node:path";

export function localManifest(args, repo, cwd = process.cwd()) {
  const end = args.indexOf("--") >= 0 ? args.indexOf("--") : args.length;
  const resolve = (value) => {
    const absolute = path.resolve(cwd, value);
    const relative = path.relative(repo, absolute);
    if (relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
      throw new Error(`--manifest-path ${value} is outside ${repo}`);
    }
    return absolute;
  };

  return args.map((arg, index) => {
    if (index >= end) return arg;
    if (arg.startsWith("--manifest-path=")) return `--manifest-path=${resolve(arg.slice("--manifest-path=".length))}`;
    if (index > 0 && args[index - 1] === "--manifest-path") return resolve(arg);
    return arg;
  });
}
