//! Disposable, bounded-memory ordering for render records. Payloads remain on disk.

use crate::{DataError, Result};
use rusqlite::{Connection, params};

pub struct Ordered {
    db: Connection,
    _file: tempfile::TempDir,
}

fn bad(e: impl std::fmt::Display) -> DataError {
    DataError::Format(format!("render ordering: {e}"))
}

impl Ordered {
    pub fn new() -> Result<Self> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("rustgis-render-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let file = builder.tempdir()?;
        let db = Connection::open(file.path().join("ordering.sqlite")).map_err(bad)?;
        db.execute_batch(
            "PRAGMA page_size=4096; PRAGMA max_page_count=1048576; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-2048; PRAGMA mmap_size=0; PRAGMA temp_store=FILE;
            CREATE TABLE shapes(seq INTEGER PRIMARY KEY, priority REAL NOT NULL, payload BLOB NOT NULL);
            CREATE INDEX sorted ON shapes(priority, seq);
            BEGIN;",
        )
        .map_err(bad)?;
        Ok(Self { db, _file: file })
    }

    pub fn insert(&self, priority: f64, payload: &[u8]) -> Result<()> {
        self.db
            .prepare_cached("INSERT INTO shapes(priority,payload) VALUES(?1,?2)")
            .map_err(bad)?
            .execute(params![priority, payload])
            .map_err(bad)?;
        Ok(())
    }

    pub fn visit(&self, mut visit: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
        let mut stmt = self.db.prepare("SELECT payload FROM shapes ORDER BY priority, seq").map_err(bad)?;
        let mut rows = stmt.query([]).map_err(bad)?;
        while let Some(row) = rows.next().map_err(bad)? {
            let value = row.get_ref(0).map_err(bad)?;
            visit(value.as_blob().map_err(bad)?)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordering_is_global_stable_bounded_and_disposable() {
        let ordered = Ordered::new().unwrap();
        let path = ordered._file.path().to_owned();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o700);
        }
        for (priority, payload) in [(2.0, b"b"), (1.0, b"a"), (2.0, b"c")] {
            ordered.insert(priority, payload).unwrap();
        }
        let mut rows = Vec::new();
        ordered
            .visit(|row| {
                rows.push(row.to_vec());
                Ok(())
            })
            .unwrap();
        assert_eq!(rows, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
        let mut calls = 0;
        assert!(
            ordered
                .visit(|_| {
                    calls += 1;
                    Err(bad("cancelled"))
                })
                .is_err()
        );
        assert_eq!(calls, 1);
        ordered.db.execute_batch("PRAGMA max_page_count=12;").unwrap();
        assert!(ordered.insert(3.0, &vec![0; 64 * 1024]).is_err());
        drop(ordered);
        assert!(!path.exists());
    }
}
