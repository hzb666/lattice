import { lstatSync, mkdirSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, sep } from "node:path";

const home = realpathSync.native(process.env.SYNARA_HOME);
const temporary = tmpdir();
const fromHome = relative(home, temporary);

if (!isAbsolute(temporary) || !fromHome || isAbsolute(fromHome) ||
    fromHome === ".." || fromHome.startsWith(`..${sep}`)) {
  throw new Error("Synara temporary directory must be inside its isolated home");
}

let directory = home;
for (const component of fromHome.split(sep)) {
  directory = join(directory, component);
  try {
    mkdirSync(directory);
  } catch (error) {
    if (error.code !== "EEXIST") throw error;
  }
  if (!lstatSync(directory).isDirectory() || relative(directory, realpathSync.native(directory))) {
    throw new Error("Synara temporary path contains a directory alias");
  }
}
