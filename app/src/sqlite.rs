// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small binding to the SQLite that ships with macOS (`libsqlite3`, with
//! FTS5 full-text search), just what the library index needs: open, run
//! statements with bound values, read rows. No crate, nothing bundled.

use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};

#[allow(non_camel_case_types)]
type sqlite3 = c_void;
#[allow(non_camel_case_types)]
type sqlite3_stmt = c_void;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const OPEN_READWRITE: c_int = 0x2;
const OPEN_CREATE: c_int = 0x4;
const OPEN_FULLMUTEX: c_int = 0x10000;

#[link(name = "sqlite3")]
unsafe extern "C" {
    fn sqlite3_open_v2(filename: *const c_char, db: *mut *mut sqlite3, flags: c_int, vfs: *const c_char) -> c_int;
    fn sqlite3_close_v2(db: *mut sqlite3) -> c_int;
    fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char;
    fn sqlite3_exec(db: *mut sqlite3, sql: *const c_char, cb: *const c_void, arg: *mut c_void, err: *mut *mut c_char) -> c_int;
    fn sqlite3_free(p: *mut c_void);
    fn sqlite3_busy_timeout(db: *mut sqlite3, ms: c_int) -> c_int;
    fn sqlite3_prepare_v2(db: *mut sqlite3, sql: *const c_char, n: c_int, stmt: *mut *mut sqlite3_stmt, tail: *mut *const c_char) -> c_int;
    fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_bind_text(stmt: *mut sqlite3_stmt, i: c_int, text: *const c_char, n: c_int, destructor: isize) -> c_int;
    fn sqlite3_bind_double(stmt: *mut sqlite3_stmt, i: c_int, v: c_double) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut sqlite3_stmt, i: c_int, v: i64) -> c_int;
    fn sqlite3_bind_null(stmt: *mut sqlite3_stmt, i: c_int) -> c_int;
    fn sqlite3_column_count(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_column_type(stmt: *mut sqlite3_stmt, i: c_int) -> c_int;
    fn sqlite3_column_text(stmt: *mut sqlite3_stmt, i: c_int) -> *const c_char;
    fn sqlite3_column_double(stmt: *mut sqlite3_stmt, i: c_int) -> c_double;
    fn sqlite3_column_int64(stmt: *mut sqlite3_stmt, i: c_int) -> i64;
}

/// SQLITE_TRANSIENT: SQLite copies bound text.
const TRANSIENT: isize = -1;

/// A value to bind or read.
#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

impl From<&str> for V {
    fn from(s: &str) -> V {
        V::Text(s.to_owned())
    }
}
impl From<String> for V {
    fn from(s: String) -> V {
        V::Text(s)
    }
}
impl From<i64> for V {
    fn from(v: i64) -> V {
        V::Int(v)
    }
}
impl From<f64> for V {
    fn from(v: f64) -> V {
        V::Real(v)
    }
}

impl V {
    pub fn text(&self) -> String {
        match self {
            V::Text(s) => s.clone(),
            V::Int(i) => i.to_string(),
            V::Real(r) => r.to_string(),
            V::Null => String::new(),
        }
    }
    pub fn real(&self) -> f64 {
        match self {
            V::Real(r) => *r,
            V::Int(i) => *i as f64,
            V::Text(s) => s.parse().unwrap_or(0.0),
            V::Null => 0.0,
        }
    }
    pub fn int(&self) -> i64 {
        match self {
            V::Int(i) => *i,
            V::Real(r) => *r as i64,
            V::Text(s) => s.parse().unwrap_or(0),
            V::Null => 0,
        }
    }
}

/// An open database. Safe to share between threads (serialized mode).
pub struct Db(*mut sqlite3);

unsafe impl Send for Db {}
unsafe impl Sync for Db {}

impl Drop for Db {
    fn drop(&mut self) {
        unsafe { sqlite3_close_v2(self.0) };
    }
}

impl Db {
    /// Opens (creating) the database at `path`, in WAL mode so the window,
    /// the command line and agents can read while one writes.
    pub fn open(path: &std::path::Path) -> Result<Db, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let c = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let mut db = std::ptr::null_mut();
        let rc = unsafe { sqlite3_open_v2(c.as_ptr(), &mut db, OPEN_READWRITE | OPEN_CREATE | OPEN_FULLMUTEX, std::ptr::null()) };
        let d = Db(db);
        if rc != SQLITE_OK {
            return Err(d.error());
        }
        unsafe { sqlite3_busy_timeout(db, 3000) };
        d.exec("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Ok(d)
    }

    fn error(&self) -> String {
        unsafe { CStr::from_ptr(sqlite3_errmsg(self.0)).to_string_lossy().into_owned() }
    }

    /// Runs one or more statements with no values and no rows.
    pub fn exec(&self, sql: &str) -> Result<(), String> {
        let c = CString::new(sql).map_err(|e| e.to_string())?;
        let mut err: *mut c_char = std::ptr::null_mut();
        let rc = unsafe { sqlite3_exec(self.0, c.as_ptr(), std::ptr::null(), std::ptr::null_mut(), &mut err) };
        if rc != SQLITE_OK {
            let msg = if err.is_null() { self.error() } else { unsafe { CStr::from_ptr(err).to_string_lossy().into_owned() } };
            if !err.is_null() {
                unsafe { sqlite3_free(err as *mut c_void) };
            }
            return Err(msg);
        }
        Ok(())
    }

    /// Runs one statement with `args`, returning every row.
    pub fn query(&self, sql: &str, args: &[V]) -> Result<Vec<Vec<V>>, String> {
        let c = CString::new(sql).map_err(|e| e.to_string())?;
        let mut st = std::ptr::null_mut();
        if unsafe { sqlite3_prepare_v2(self.0, c.as_ptr(), -1, &mut st, std::ptr::null_mut()) } != SQLITE_OK {
            return Err(self.error());
        }
        struct Fin(*mut sqlite3_stmt);
        impl Drop for Fin {
            fn drop(&mut self) {
                unsafe { sqlite3_finalize(self.0) };
            }
        }
        let st = Fin(st);
        for (i, a) in args.iter().enumerate() {
            let i = i as c_int + 1;
            let rc = match a {
                V::Null => unsafe { sqlite3_bind_null(st.0, i) },
                V::Int(v) => unsafe { sqlite3_bind_int64(st.0, i, *v) },
                V::Real(v) => unsafe { sqlite3_bind_double(st.0, i, *v) },
                V::Text(s) => unsafe { sqlite3_bind_text(st.0, i, s.as_ptr() as *const c_char, s.len() as c_int, TRANSIENT) },
            };
            if rc != SQLITE_OK {
                return Err(self.error());
            }
        }
        let mut rows = Vec::new();
        loop {
            match unsafe { sqlite3_step(st.0) } {
                SQLITE_ROW => {
                    let n = unsafe { sqlite3_column_count(st.0) };
                    let row = (0..n)
                        .map(|i| match unsafe { sqlite3_column_type(st.0, i) } {
                            1 => V::Int(unsafe { sqlite3_column_int64(st.0, i) }),
                            2 => V::Real(unsafe { sqlite3_column_double(st.0, i) }),
                            5 => V::Null,
                            _ => {
                                let p = unsafe { sqlite3_column_text(st.0, i) };
                                if p.is_null() { V::Null } else { V::Text(unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() }) }
                            }
                        })
                        .collect();
                    rows.push(row);
                }
                SQLITE_DONE => return Ok(rows),
                _ => return Err(self.error()),
            }
        }
    }

    /// Runs one statement with `args`, ignoring rows.
    pub fn run(&self, sql: &str, args: &[V]) -> Result<(), String> {
        self.query(sql, args).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_text_search_works() {
        let path = std::env::temp_dir().join(format!("studio-sqlite-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Db::open(&path).unwrap();
        db.exec("CREATE VIRTUAL TABLE t USING fts5(kind, body)").unwrap();
        db.run("INSERT INTO t VALUES (?, ?)", &["spoken".into(), "Zornov renders five times faster".into()]).unwrap();
        db.run("INSERT INTO t VALUES (?, ?)", &["title".into(), "The render race".into()]).unwrap();
        let hits = db.query("SELECT kind FROM t WHERE t MATCH ? ORDER BY rank", &["render*".into()]).unwrap();
        assert_eq!(hits.len(), 2);
        let none = db.query("SELECT kind FROM t WHERE t MATCH ?", &["glimmers".into()]).unwrap();
        assert!(none.is_empty());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
