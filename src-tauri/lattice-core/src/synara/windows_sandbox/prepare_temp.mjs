import { lstatSync, mkdirSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, sep } from "node:path";
import { DatabaseSync } from "node:sqlite";

const home = realpathSync.native(process.env.SYNARA_HOME);
const temporary = tmpdir();
const fromHome = relative(home, temporary);

if (!isAbsolute(temporary) || !fromHome || isAbsolute(fromHome) ||
    fromHome === ".." || fromHome.startsWith(`..${sep}`)) {
  throw new Error("Synara temporary directory must be inside its isolated home");
}

function ensureOrdinaryDirectory(directory) {
  try {
    mkdirSync(directory);
  } catch (error) {
    if (error.code !== "EEXIST") throw error;
  }
  if (!lstatSync(directory).isDirectory() || relative(directory, realpathSync.native(directory))) {
    throw new Error("Synara temporary path contains a directory alias");
  }
}

let directory = home;
for (const component of fromHome.split(sep)) {
  directory = join(directory, component);
  ensureOrdinaryDirectory(directory);
}

const sqliteTemp = join(home, "temp");
ensureOrdinaryDirectory(sqliteTemp);
// MXC redirects OS temp; SQLite migration spill files need the writable home.
const database = new DatabaseSync(":memory:");
try {
  const { path } = database.prepare("SELECT quote(?) AS path").get(sqliteTemp);
  database.exec(`PRAGMA temp_store_directory = ${path}`);
} finally {
  database.close();
}
