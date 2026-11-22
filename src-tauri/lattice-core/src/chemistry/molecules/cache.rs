use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

use chemex_cli::StructureReport;
use rusqlite::{params, Connection};

pub(super) const DB_RELATIVE: &str = ".chem/molecules.sqlite";
const SCHEMA_VERSION: i64 = 2;

pub(super) type FileStamp = (i64, i64);

pub(super) struct CachedDrawing {
    stamp: FileStamp,
    json: String,
}

impl CachedDrawing {
    pub(super) fn report_at(self, stamp: Option<FileStamp>) -> Option<StructureReport> {
        if stamp != Some(self.stamp) {
            return None;
        }
        serde_json::from_str(&self.json).ok()
    }
}

pub(super) fn file_stamp(root: &Path, relative: &str) -> Option<FileStamp> {
    // shortcut: identical mtime and size can hide edits; use a content fingerprint for strict invalidation.
    let path = crate::project::safe_path(root, relative).ok()?;
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((i64::try_from(modified.as_nanos()).ok()?, i64::try_from(metadata.len()).ok()?))
}

pub(super) fn open(root: &Path) -> Result<Connection, String> {
    let path = crate::project::creation_path(root, DB_RELATIVE)?;
    let mut connection = Connection::open(path).map_err(crate::util::err)?;
    connection.busy_timeout(Duration::from_secs(5)).map_err(crate::util::err)?;
    let version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(crate::util::err)?;
    match version {
        SCHEMA_VERSION => Ok(connection),
        0 => {
            let transaction = connection.transaction().map_err(crate::util::err)?;
            transaction
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS drawing_analysis (
                        path TEXT PRIMARY KEY NOT NULL,
                        modified_ns INTEGER NOT NULL,
                        size INTEGER NOT NULL,
                        report TEXT NOT NULL
                     );
                     DROP TABLE IF EXISTS analysis;
                     DROP TABLE IF EXISTS source_files;
                     DROP TABLE IF EXISTS cache_meta;",
                )
                .map_err(crate::util::err)?;
            transaction
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(crate::util::err)?;
            transaction.commit().map_err(crate::util::err)?;
            Ok(connection)
        }
        _ => Err("Unsupported molecule cache version".into()),
    }
}

pub(super) fn load(connection: &Connection) -> rusqlite::Result<HashMap<String, CachedDrawing>> {
    let mut statement =
        connection.prepare("SELECT path, modified_ns, size, report FROM drawing_analysis")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get(0)?, CachedDrawing { stamp: (row.get(1)?, row.get(2)?), json: row.get(3)? }))
    })?;
    rows.collect()
}

pub(super) fn store<'a>(
    connection: &mut Connection, removed: impl Iterator<Item = &'a str>,
    updates: &[(&str, FileStamp, &StructureReport)],
) -> Result<(), String> {
    let mut removed = removed.peekable();
    if updates.is_empty() && removed.peek().is_none() {
        return Ok(());
    }
    let transaction = connection.transaction().map_err(crate::util::err)?;
    for path in removed {
        transaction
            .execute("DELETE FROM drawing_analysis WHERE path = ?1", [path])
            .map_err(crate::util::err)?;
    }
    for (path, (modified_ns, size), report) in updates {
        let json = serde_json::to_string(report).map_err(crate::util::err)?;
        transaction.execute(
            "INSERT INTO drawing_analysis(path, modified_ns, size, report) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET modified_ns = excluded.modified_ns,
                 size = excluded.size, report = excluded.report",
            params![path, modified_ns, size, json],
        ).map_err(crate::util::err)?;
    }
    transaction.commit().map_err(crate::util::err)
}
