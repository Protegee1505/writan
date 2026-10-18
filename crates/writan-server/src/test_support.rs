//! Helpers shared by the unit tests (compiled only under `cfg(test)`).

use std::path::Path;

use rusqlite::Connection;

use crate::db;

/// An in-memory database with the full schema applied.
pub fn memory_db() -> Connection {
    db::open(Path::new(":memory:")).expect("in-memory database")
}
