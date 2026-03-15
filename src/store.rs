use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LibraryId {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
}

impl LibraryId {
    pub fn to_string_id(&self) -> String {
        format!("{}:{}:{}", self.group_id, self.artifact_id, self.version)
    }

    pub fn db_path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir
            .join(&self.group_id)
            .join(&self.artifact_id)
            .join(format!("{}.db", self.version))
    }
}

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct PackageRow {
    pub id: i64,
    pub name: String,
    pub doc_comment: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TypeRow {
    pub id: i64,
    pub package_id: i64,
    pub name: String,
    pub fqn: String,
    pub kind: String,
    pub doc_comment: Option<String>,
    pub annotations: Option<String>,
    pub superclass: Option<String>,
    pub interfaces: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MethodRow {
    pub id: i64,
    pub type_id: i64,
    pub name: String,
    pub signature: String,
    pub return_type: Option<String>,
    pub params: String,
    pub doc_comment: Option<String>,
    pub annotations: Option<String>,
    pub is_static: bool,
}

#[derive(Debug, Clone)]
pub struct FieldRow {
    pub id: i64,
    pub type_id: i64,
    pub name: String,
    pub field_type: String,
    pub doc_comment: Option<String>,
    pub annotations: Option<String>,
    pub is_static: bool,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub fqn: String,
    pub kind: String,
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
    pub rank: f64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.create_schema()?;
        Ok(store)
    }

    pub fn open_if_exists(path: &Path) -> Result<Option<Self>> {
        if path.exists() {
            Ok(Some(Self::open(path)?))
        } else {
            Ok(None)
        }
    }

    fn create_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS library (
                group_id    TEXT NOT NULL,
                artifact_id TEXT NOT NULL,
                version     TEXT NOT NULL,
                source_type TEXT NOT NULL,
                indexed_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS package (
                id          INTEGER PRIMARY KEY,
                name        TEXT NOT NULL UNIQUE,
                doc_comment TEXT
            );

            CREATE TABLE IF NOT EXISTS type (
                id          INTEGER PRIMARY KEY,
                package_id  INTEGER NOT NULL REFERENCES package(id),
                name        TEXT NOT NULL,
                fqn         TEXT NOT NULL UNIQUE,
                kind        TEXT NOT NULL,
                doc_comment TEXT,
                annotations TEXT,
                superclass  TEXT,
                interfaces  TEXT
            );

            CREATE TABLE IF NOT EXISTS method (
                id          INTEGER PRIMARY KEY,
                type_id     INTEGER NOT NULL REFERENCES type(id),
                name        TEXT NOT NULL,
                signature   TEXT NOT NULL,
                return_type TEXT,
                params      TEXT NOT NULL,
                doc_comment TEXT,
                annotations TEXT,
                is_static   INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS field (
                id          INTEGER PRIMARY KEY,
                type_id     INTEGER NOT NULL REFERENCES type(id),
                name        TEXT NOT NULL,
                field_type  TEXT NOT NULL,
                doc_comment TEXT,
                annotations TEXT,
                is_static   INTEGER NOT NULL DEFAULT 0
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS docs_fts USING fts5(
                fqn,
                kind,
                signature,
                doc_comment
            );

            CREATE INDEX IF NOT EXISTS idx_type_package ON type(package_id);
            CREATE INDEX IF NOT EXISTS idx_method_type ON method(type_id);
            CREATE INDEX IF NOT EXISTS idx_field_type ON field(type_id);
            "
        )?;
        Ok(())
    }

    pub fn set_library_meta(&self, lib: &LibraryId, source_type: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO library (group_id, artifact_id, version, source_type, indexed_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![lib.group_id, lib.artifact_id, lib.version, source_type],
        )?;
        Ok(())
    }

    pub fn has_library_meta(&self) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM library",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn insert_package(&self, name: &str, doc_comment: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO package (name, doc_comment) VALUES (?1, ?2)",
            params![name, doc_comment],
        )?;
        let id = self.conn.query_row(
            "SELECT id FROM package WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn begin_transaction(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        Ok(())
    }

    pub fn commit_transaction(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    fn insert_fts(&self, fqn: &str, kind: &str, signature: &str, doc_comment: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO docs_fts (fqn, kind, signature, doc_comment) VALUES (?1, ?2, ?3, ?4)",
            params![fqn, kind, signature, doc_comment],
        )?;
        Ok(())
    }

    pub fn insert_type(&self, row: &TypeRow) -> Result<i64> {
        self.conn.execute(
            "INSERT OR REPLACE INTO type (package_id, name, fqn, kind, doc_comment, annotations, superclass, interfaces)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.package_id, row.name, row.fqn, row.kind,
                row.doc_comment, row.annotations, row.superclass, row.interfaces
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.insert_fts(&row.fqn, &row.kind, "", row.doc_comment.as_deref())?;
        Ok(id)
    }

    /// Insert a method. `parent_fqn` is the owning type's fully qualified name.
    pub fn insert_method(&self, row: &MethodRow, parent_fqn: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO method (type_id, name, signature, return_type, params, doc_comment, annotations, is_static)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.type_id, row.name, row.signature, row.return_type,
                row.params, row.doc_comment, row.annotations, row.is_static as i32
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        let method_fqn = format!("{}.{}", parent_fqn, row.name);
        self.insert_fts(&method_fqn, "method", &row.signature, row.doc_comment.as_deref())?;
        Ok(id)
    }

    /// Insert a field. `parent_fqn` is the owning type's fully qualified name.
    pub fn insert_field(&self, row: &FieldRow, parent_fqn: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO field (type_id, name, field_type, doc_comment, annotations, is_static)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.type_id, row.name, row.field_type,
                row.doc_comment, row.annotations, row.is_static as i32
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        let field_fqn = format!("{}.{}", parent_fqn, row.name);
        self.insert_fts(&field_fqn, "field", &row.field_type, row.doc_comment.as_deref())?;
        Ok(id)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let mut stmt = self.conn.prepare(
            "SELECT fqn, kind, signature, doc_comment, bm25(docs_fts, 10.0, 5.0, 3.0, 1.0) as rank
             FROM docs_fts
             WHERE docs_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2"
        )?;
        let results = stmt.query_map(params![query, limit as i64], |row| {
            Ok(SearchResult {
                fqn: row.get(0)?,
                kind: row.get(1)?,
                signature: row.get(2)?,
                doc_comment: row.get(3)?,
                rank: row.get(4)?,
            })
        })?;
        results.into_iter().map(|r| r.map_err(Into::into)).collect()
    }

    pub fn list_packages(&self) -> Result<Vec<PackageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, doc_comment FROM package ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PackageRow {
                id: row.get(0)?,
                name: row.get(1)?,
                doc_comment: row.get(2)?,
            })
        })?;
        rows.into_iter().map(|r| r.map_err(Into::into)).collect()
    }

    pub fn list_types_in_package(&self, package_name: &str) -> Result<Vec<TypeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.package_id, t.name, t.fqn, t.kind, t.doc_comment, t.annotations, t.superclass, t.interfaces
             FROM type t
             JOIN package p ON t.package_id = p.id
             WHERE p.name = ?1
             ORDER BY t.name"
        )?;
        let rows = stmt.query_map(params![package_name], |row| {
            Ok(TypeRow {
                id: row.get(0)?,
                package_id: row.get(1)?,
                name: row.get(2)?,
                fqn: row.get(3)?,
                kind: row.get(4)?,
                doc_comment: row.get(5)?,
                annotations: row.get(6)?,
                superclass: row.get(7)?,
                interfaces: row.get(8)?,
            })
        })?;
        rows.into_iter().map(|r| r.map_err(Into::into)).collect()
    }

    pub fn get_type_by_fqn(&self, fqn: &str) -> Result<Option<TypeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, package_id, name, fqn, kind, doc_comment, annotations, superclass, interfaces
             FROM type WHERE fqn = ?1"
        )?;
        let mut rows = stmt.query_map(params![fqn], |row| {
            Ok(TypeRow {
                id: row.get(0)?,
                package_id: row.get(1)?,
                name: row.get(2)?,
                fqn: row.get(3)?,
                kind: row.get(4)?,
                doc_comment: row.get(5)?,
                annotations: row.get(6)?,
                superclass: row.get(7)?,
                interfaces: row.get(8)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn get_methods_for_type(&self, type_id: i64) -> Result<Vec<MethodRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, type_id, name, signature, return_type, params, doc_comment, annotations, is_static
             FROM method WHERE type_id = ?1 ORDER BY name"
        )?;
        let rows = stmt.query_map(params![type_id], |row| {
            Ok(MethodRow {
                id: row.get(0)?,
                type_id: row.get(1)?,
                name: row.get(2)?,
                signature: row.get(3)?,
                return_type: row.get(4)?,
                params: row.get(5)?,
                doc_comment: row.get(6)?,
                annotations: row.get(7)?,
                is_static: row.get::<_, i32>(8)? != 0,
            })
        })?;
        rows.into_iter().map(|r| r.map_err(Into::into)).collect()
    }

    pub fn get_fields_for_type(&self, type_id: i64) -> Result<Vec<FieldRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, type_id, name, field_type, doc_comment, annotations, is_static
             FROM field WHERE type_id = ?1 ORDER BY name"
        )?;
        let rows = stmt.query_map(params![type_id], |row| {
            Ok(FieldRow {
                id: row.get(0)?,
                type_id: row.get(1)?,
                name: row.get(2)?,
                field_type: row.get(3)?,
                doc_comment: row.get(4)?,
                annotations: row.get(5)?,
                is_static: row.get::<_, i32>(6)? != 0,
            })
        })?;
        rows.into_iter().map(|r| r.map_err(Into::into)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn test_store() -> (Store, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        (store, tmp)
    }

    #[test]
    fn test_schema_creation() {
        let (store, _tmp) = test_store();
        assert!(!store.has_library_meta().unwrap());
    }

    #[test]
    fn test_insert_and_query_package() {
        let (store, _tmp) = test_store();
        let pkg_id = store.insert_package("com.example", Some("Example package")).unwrap();
        let packages = store.list_packages().unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "com.example");
        assert_eq!(packages[0].id, pkg_id);
    }

    #[test]
    fn test_insert_type_and_search() {
        let (store, _tmp) = test_store();
        let pkg_id = store.insert_package("com.example", None).unwrap();
        let type_row = TypeRow {
            id: 0,
            package_id: pkg_id,
            name: "MyClass".to_string(),
            fqn: "com.example.MyClass".to_string(),
            kind: "class".to_string(),
            doc_comment: Some("A sample class for testing".to_string()),
            annotations: None,
            superclass: None,
            interfaces: None,
        };
        store.insert_type(&type_row).unwrap();

        let results = store.search("sample class", 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].fqn, "com.example.MyClass");
    }

    #[test]
    fn test_insert_method_and_browse() {
        let (store, _tmp) = test_store();
        let pkg_id = store.insert_package("com.example", None).unwrap();
        let type_row = TypeRow {
            id: 0, package_id: pkg_id, name: "MyClass".to_string(),
            fqn: "com.example.MyClass".to_string(), kind: "class".to_string(),
            doc_comment: None, annotations: None, superclass: None, interfaces: None,
        };
        let type_id = store.insert_type(&type_row).unwrap();
        let method_row = MethodRow {
            id: 0, type_id, name: "doSomething".to_string(),
            signature: "public void doSomething(String input)".to_string(),
            return_type: Some("void".to_string()),
            params: r#"[{"name":"input","type":"String"}]"#.to_string(),
            doc_comment: Some("Does something useful".to_string()),
            annotations: None, is_static: false,
        };
        store.insert_method(&method_row, "com.example.MyClass").unwrap();

        let methods = store.get_methods_for_type(type_id).unwrap();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, "doSomething");

        let results = store.search("something useful", 10).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_browse_type_by_fqn() {
        let (store, _tmp) = test_store();
        let pkg_id = store.insert_package("com.example", None).unwrap();
        let type_row = TypeRow {
            id: 0, package_id: pkg_id, name: "Foo".to_string(),
            fqn: "com.example.Foo".to_string(), kind: "interface".to_string(),
            doc_comment: Some("A foo interface".to_string()), annotations: None,
            superclass: None, interfaces: None,
        };
        store.insert_type(&type_row).unwrap();

        let found = store.get_type_by_fqn("com.example.Foo").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().kind, "interface");

        let not_found = store.get_type_by_fqn("com.example.Bar").unwrap();
        assert!(not_found.is_none());
    }
}
