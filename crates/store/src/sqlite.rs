//! SQLite-backed [`DocumentStore`].

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use shared::{
    Document, DocumentStore, DocumentTransaction, FunctionFailure, JsonValue, MutateOutcome,
    Request, apply_mutation,
};

/// Schema version stored in `PRAGMA user_version`.
pub const DB_SCHEMA_VERSION: i64 = 1;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const CREATE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS documents (
    collection TEXT PRIMARY KEY,
    version    INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    body       TEXT NOT NULL
);";

/// SQLite implementation of [`DocumentStore`].
pub struct SqliteStore {
    connection: Mutex<Connection>,
}

impl SqliteStore {
    /// Open (or create) a database file, creating the parent directory `0700`
    /// and the database file `0600` on unix hosts.
    pub fn open(path: &Path) -> Result<Self, FunctionFailure> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let created = !parent.exists();
            std::fs::create_dir_all(parent).map_err(|error| {
                FunctionFailure::new(
                    "INTERNAL_ERROR",
                    format!("could not create directory {}: {error}", parent.display()),
                )
            })?;
            if created {
                restrict_permissions(parent, 0o700)?;
            }
        }
        let created = !path.exists();
        let connection = Connection::open(path).map_err(|error| db_failure("open", error))?;
        if created {
            restrict_permissions(path, 0o600)?;
        }
        Self::prepare(connection, true)
    }

    /// Open an existing database without creating one. Read-only commands
    /// (`check`, `backup`, `serve`) use this so a typo cannot leave an empty
    /// database behind.
    pub fn open_existing(path: &Path) -> Result<Self, FunctionFailure> {
        if !path.is_file() {
            return Err(FunctionFailure::not_found(format!(
                "database {} does not exist; run `me-server init --db {}` or `me-server import --from <ME repo>` first",
                path.display(),
                path.display()
            )));
        }
        let connection =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(|error| db_failure("open", error))?;
        Self::prepare(connection, true)
    }

    /// Open an isolated in-memory database, used by tests.
    pub fn open_in_memory() -> Result<Self, FunctionFailure> {
        let connection = Connection::open_in_memory().map_err(|error| db_failure("open", error))?;
        Self::prepare(connection, false)
    }

    /// Schema version recorded in `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<i64, FunctionFailure> {
        let connection = self.guard()?;
        connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|error| db_failure("user_version", error))
    }

    /// Write a consistent snapshot via `VACUUM INTO`. Refuses to overwrite.
    pub fn backup_to(&self, target: &Path) -> Result<(), FunctionFailure> {
        if target.exists() {
            return Err(FunctionFailure::invalid_input(
                "backup target already exists; remove it or choose another path",
            ));
        }
        if let Some(parent) = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let created = !parent.exists();
            std::fs::create_dir_all(parent).map_err(|error| {
                FunctionFailure::new(
                    "INTERNAL_ERROR",
                    format!("could not create directory {}: {error}", parent.display()),
                )
            })?;
            if created {
                restrict_permissions(parent, 0o700)?;
            }
        }
        let connection = self.guard()?;
        connection
            .execute("VACUUM INTO ?1", params![target.to_string_lossy()])
            .map_err(|error| db_failure("backup", error))?;
        restrict_permissions(target, 0o600)?;
        Ok(())
    }

    fn prepare(connection: Connection, file_backed: bool) -> Result<Self, FunctionFailure> {
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| db_failure("busy_timeout", error))?;
        if file_backed {
            let _: String = connection
                .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
                .map_err(|error| db_failure("journal_mode", error))?;
            connection
                .execute_batch(
                    "PRAGMA synchronous = NORMAL;
                     PRAGMA wal_autocheckpoint = 1000;
                     PRAGMA journal_size_limit = 67108864;
                     PRAGMA cache_size = -32000;
                     PRAGMA temp_store = MEMORY;",
                )
                .map_err(|error| db_failure("pragmas", error))?;
        }
        connection
            .execute_batch(&format!(
                "PRAGMA user_version = {DB_SCHEMA_VERSION};
{CREATE_SCHEMA}"
            ))
            .map_err(|error| db_failure("schema", error))?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn guard(&self) -> Result<MutexGuard<'_, Connection>, FunctionFailure> {
        self.connection.lock().map_err(|_| {
            FunctionFailure::new("INTERNAL_ERROR", "sqlite connection lock is poisoned")
        })
    }
}

impl DocumentStore for SqliteStore {
    fn collections(&self) -> Result<Vec<String>, FunctionFailure> {
        let connection = self.guard()?;
        let mut statement = connection
            .prepare("SELECT collection FROM documents ORDER BY collection")
            .map_err(|error| db_failure("collections", error))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| db_failure("collections", error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| db_failure("collections", error))
    }

    fn load(&self, collection: &str) -> Result<Document, FunctionFailure> {
        self.load_optional(collection)?.ok_or_else(|| {
            FunctionFailure::not_found(format!("collection {collection} does not exist"))
        })
    }

    fn load_optional(&self, collection: &str) -> Result<Option<Document>, FunctionFailure> {
        let connection = self.guard()?;
        load_locked(&connection, collection)
    }

    fn version(&self, collection: &str) -> Result<Option<u64>, FunctionFailure> {
        let connection = self.guard()?;
        connection
            .query_row(
                "SELECT version FROM documents WHERE collection = ?1",
                params![collection],
                |row| row.get::<_, i64>(0).map(|version| version.max(0) as u64),
            )
            .optional()
            .map_err(|error| db_failure("version", error))
    }

    fn insert_initial(
        &self,
        collection: &str,
        document: &Document,
    ) -> Result<bool, FunctionFailure> {
        let connection = self.guard()?;
        let body = serialize(document);
        let changes = connection
            .execute(
                "INSERT OR IGNORE INTO documents (collection, version, updated_at, body)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    collection,
                    document_version(document),
                    document_updated_at(document),
                    body
                ],
            )
            .map_err(|error| db_failure(collection, error))?;
        Ok(changes > 0)
    }

    fn replace(&self, collection: &str, document: &Document) -> Result<(), FunctionFailure> {
        let connection = self.guard()?;
        write_locked(&connection, collection, document)
    }

    fn remove(&self, collection: &str) -> Result<bool, FunctionFailure> {
        let connection = self.guard()?;
        remove_locked(&connection, collection)
    }

    fn transaction(
        &self,
        body: &mut dyn FnMut(&mut dyn DocumentTransaction) -> Result<JsonValue, FunctionFailure>,
    ) -> Result<JsonValue, FunctionFailure> {
        let connection = self.guard()?;
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| db_failure("transaction", error))?;

        let result = {
            let mut transaction = SqliteTransaction {
                connection: &connection,
            };
            body(&mut transaction)
        };

        match result {
            Ok(value) => {
                if let Err(error) = connection.execute_batch("COMMIT") {
                    // A failed COMMIT leaves the transaction open; roll back so the
                    // pooled connection is not stuck inside a transaction.
                    let _ = connection.execute_batch("ROLLBACK");
                    return Err(db_failure("commit", error));
                }
                Ok(value)
            }
            Err(error) => {
                let _ = connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
}

/// Transaction view over one SQLite connection.
struct SqliteTransaction<'a> {
    connection: &'a Connection,
}

impl DocumentTransaction for SqliteTransaction<'_> {
    fn load(&mut self, collection: &str) -> Result<Document, FunctionFailure> {
        self.load_optional(collection)?.ok_or_else(|| {
            FunctionFailure::not_found(format!("collection {collection} does not exist"))
        })
    }

    fn load_optional(&mut self, collection: &str) -> Result<Option<Document>, FunctionFailure> {
        load_locked(self.connection, collection)
    }

    fn mutate(
        &mut self,
        collection: &str,
        request: &Request,
        operation: &str,
        mutate: &mut dyn FnMut(&mut Document) -> Result<JsonValue, FunctionFailure>,
    ) -> Result<MutateOutcome, FunctionFailure> {
        let mut document = match load_locked(self.connection, collection)? {
            Some(document) => document,
            None if request.expected_version == 0 => Document::new(),
            None => {
                return Err(FunctionFailure::not_found(format!(
                    "collection {collection} does not exist"
                )));
            }
        };
        let outcome = apply_mutation(&mut document, request, operation, mutate)?;
        if matches!(outcome, MutateOutcome::Applied { .. }) {
            write_locked(self.connection, collection, &document)?;
        }
        Ok(outcome)
    }

    fn insert_initial(
        &mut self,
        collection: &str,
        document: &Document,
    ) -> Result<bool, FunctionFailure> {
        let changes = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO documents (collection, version, updated_at, body)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    collection,
                    document_version(document),
                    document_updated_at(document),
                    serialize(document)
                ],
            )
            .map_err(|error| db_failure(collection, error))?;
        Ok(changes > 0)
    }

    fn replace(&mut self, collection: &str, document: &Document) -> Result<(), FunctionFailure> {
        write_locked(self.connection, collection, document)
    }

    fn remove(&mut self, collection: &str) -> Result<bool, FunctionFailure> {
        remove_locked(self.connection, collection)
    }
}

fn remove_locked(connection: &Connection, collection: &str) -> Result<bool, FunctionFailure> {
    let changes = connection
        .execute(
            "DELETE FROM documents WHERE collection = ?1",
            params![collection],
        )
        .map_err(|error| db_failure(collection, error))?;
    Ok(changes > 0)
}

fn load_locked(
    connection: &Connection,
    collection: &str,
) -> Result<Option<Document>, FunctionFailure> {
    let body = connection
        .query_row(
            "SELECT body FROM documents WHERE collection = ?1",
            params![collection],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| db_failure(collection, error))?;
    body.map(|body| parse_document(collection, &body))
        .transpose()
}

fn write_locked(
    connection: &Connection,
    collection: &str,
    document: &Document,
) -> Result<(), FunctionFailure> {
    connection
        .execute(
            "INSERT OR REPLACE INTO documents (collection, version, updated_at, body)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                collection,
                document_version(document),
                document_updated_at(document),
                serialize(document)
            ],
        )
        .map(|_| ())
        .map_err(|error| db_failure(collection, error))
}

fn serialize(document: &Document) -> String {
    shared::json::object_to_json_string(document)
}

fn parse_document(collection: &str, body: &str) -> Result<Document, FunctionFailure> {
    match shared::parse_json(body) {
        Ok(JsonValue::Object(document)) => Ok(document),
        _ => Err(FunctionFailure::new(
            "DATA_INVALID",
            format!("collection {collection} does not contain a JSON object"),
        )),
    }
}

fn document_version(document: &Document) -> i64 {
    document
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0) as i64
}

fn document_updated_at(document: &Document) -> String {
    document
        .get("updated_at")
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .to_string()
}

fn restrict_permissions(path: &Path, mode: u32) -> Result<(), FunctionFailure> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|error| {
            FunctionFailure::new(
                "INTERNAL_ERROR",
                format!("could not set permissions on {}: {error}", path.display()),
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn db_failure(context: &str, error: rusqlite::Error) -> FunctionFailure {
    if let rusqlite::Error::SqliteFailure(code, _) = &error
        && matches!(
            code.code,
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
        )
    {
        return FunctionFailure::new(
            "WRITE_BUSY",
            format!("database is busy while accessing {context}: {error}"),
        );
    }
    FunctionFailure::new(
        "INTERNAL_ERROR",
        format!("database error while accessing {context}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::SqliteStore;
    use shared::{DocumentStore, FunctionFailure, MutateOutcome, Request};

    #[test]
    fn open_creates_schema_and_persists_documents() {
        let store = SqliteStore::open_in_memory().expect("open");
        assert_eq!(store.schema_version().unwrap(), 1);
        let mut doc = shared::Document::new();
        doc.insert("version".into(), shared::JsonValue::from(1_u64));
        store.insert_initial("tasks", &doc).unwrap();
        assert_eq!(store.load("tasks").unwrap(), doc);
    }

    #[cfg(unix)]
    #[test]
    fn open_keeps_existing_directory_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::path::PathBuf::from("target/test-tmp/store-existing-dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        let store = SqliteStore::open(&dir.join("me.db")).expect("open");
        drop(store);

        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "pre-existing directories keep their mode");
    }

    #[cfg(unix)]
    #[test]
    fn open_creates_private_directory_and_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::path::PathBuf::from("target/test-tmp/store-perms");
        let _ = std::fs::remove_dir_all(&dir);
        let store = SqliteStore::open(&dir.join("me.db")).expect("open");
        drop(store);
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(dir.join("me.db"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn mutate_error_rolls_back_transaction() {
        let store = SqliteStore::open_in_memory().expect("open");
        let mut doc = shared::Document::new();
        doc.insert("version".into(), shared::JsonValue::from(1_u64));
        store.insert_initial("tasks", &doc).unwrap();
        let request = Request {
            request_id: "r1".into(),
            expected_version: 1,
            input: Default::default(),
        };
        let error = store
            .with_document_mut("tasks", &request, "task.create", &mut |doc| {
                doc.insert("title".into(), shared::JsonValue::from("half"));
                Err(FunctionFailure::invalid_input("boom"))
            })
            .unwrap_err();
        assert_eq!(error.code, "INVALID_INPUT");
        assert_eq!(store.load("tasks").unwrap(), doc);
    }

    #[test]
    fn receipts_survive_reopening_the_database() {
        let dir = std::path::PathBuf::from("target/test-tmp/store-receipts");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("me.db");
        let mut doc = shared::Document::new();
        doc.insert("version".into(), shared::JsonValue::from(1_u64));
        {
            let store = SqliteStore::open(&path).unwrap();
            store.insert_initial("tasks", &doc).unwrap();
            let request = Request {
                request_id: "req-keep".into(),
                expected_version: 1,
                input: Default::default(),
            };
            store
                .with_document_mut("tasks", &request, "task.create", &mut |_| {
                    Ok(shared::JsonValue::object([(
                        "tag",
                        shared::JsonValue::from("saved"),
                    )]))
                })
                .unwrap();
            let replay = store.with_document_mut("tasks", &request, "task.create", &mut |_| {
                panic!("must not run")
            });
            assert!(matches!(replay.unwrap(), MutateOutcome::Replayed(_)));
        }
        let reopened = SqliteStore::open(&path).unwrap();
        let replay = reopened.with_document_mut(
            "tasks",
            &Request {
                request_id: "req-keep".into(),
                expected_version: 1,
                input: Default::default(),
            },
            "task.create",
            &mut |_| panic!("must not run"),
        );
        assert!(matches!(replay.unwrap(), MutateOutcome::Replayed(_)));
    }

    #[test]
    fn concurrent_writers_are_serialized() {
        let dir = std::path::PathBuf::from("target/test-tmp/store-concurrent");
        let _ = std::fs::remove_dir_all(&dir);
        let store = std::sync::Arc::new(SqliteStore::open(&dir.join("me.db")).unwrap());
        let mut doc = shared::Document::new();
        doc.insert("version".into(), shared::JsonValue::from(1_u64));
        store.insert_initial("tasks", &doc).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|worker| {
                let store = std::sync::Arc::clone(&store);
                std::thread::spawn(move || {
                    for step in 0..10 {
                        loop {
                            let current = store
                                .load("tasks")
                                .unwrap()
                                .get("version")
                                .and_then(shared::JsonValue::as_u64)
                                .unwrap();
                            let request = Request {
                                request_id: format!("req-{worker}-{step}"),
                                expected_version: current,
                                input: Default::default(),
                            };
                            match store.with_document_mut(
                                "tasks",
                                &request,
                                "task.create",
                                &mut |doc| {
                                    doc.insert(
                                        "items".into(),
                                        shared::JsonValue::Array(Vec::new()),
                                    );
                                    Ok(shared::JsonValue::Object(Default::default()))
                                },
                            ) {
                                Ok(_) => break,
                                Err(error) if error.code == "VERSION_CONFLICT" => continue,
                                Err(error) => panic!("unexpected: {}", error.message),
                            }
                        }
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            store
                .load("tasks")
                .unwrap()
                .get("version")
                .and_then(shared::JsonValue::as_u64),
            Some(41)
        );
    }

    #[test]
    fn open_existing_does_not_create_a_database() {
        let dir = std::path::PathBuf::from("target/test-tmp/store-open-existing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("missing.db");

        let error = match SqliteStore::open_existing(&path) {
            Ok(_) => panic!("missing database must not open"),
            Err(error) => error,
        };
        assert_eq!(error.code, "NOT_FOUND");
        assert!(!path.exists(), "open_existing must not create the file");
    }

    #[test]
    fn open_existing_reads_a_created_database() {
        let dir = std::path::PathBuf::from("target/test-tmp/store-open-existing-ok");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("me.db");
        {
            let store = SqliteStore::open(&path).unwrap();
            let mut doc = shared::Document::new();
            doc.insert("version".into(), shared::JsonValue::from(4_u64));
            store.insert_initial("tasks", &doc).unwrap();
        }

        let store = SqliteStore::open_existing(&path).unwrap();
        assert_eq!(
            store
                .load("tasks")
                .unwrap()
                .get("version")
                .and_then(shared::JsonValue::as_u64),
            Some(4)
        );
    }

    #[test]
    fn transactions_serialize_and_see_committed_writes() {
        let dir = std::path::PathBuf::from("target/test-tmp/store-serialized");
        let _ = std::fs::remove_dir_all(&dir);
        let store = std::sync::Arc::new(SqliteStore::open(&dir.join("me.db")).unwrap());
        let mut doc = shared::Document::new();
        doc.insert("version".into(), shared::JsonValue::from(1_u64));
        store.insert_initial("tasks", &doc).unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let writer = {
            let store = std::sync::Arc::clone(&store);
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                store
                    .transaction(&mut |transaction| {
                        transaction.load("tasks")?;
                        barrier.wait();
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        transaction.mutate(
                            "tasks",
                            &Request {
                                request_id: "req-first".into(),
                                expected_version: 1,
                                input: Default::default(),
                            },
                            "task.create",
                            &mut |document| {
                                document.insert("first".into(), shared::JsonValue::from(true));
                                Ok(shared::JsonValue::Object(Default::default()))
                            },
                        )?;
                        Ok(shared::JsonValue::Null)
                    })
                    .expect("first transaction");
            })
        };

        barrier.wait();
        store
            .transaction(&mut |transaction| {
                let tasks = transaction.load("tasks")?;
                assert_eq!(
                    tasks.get("version").and_then(shared::JsonValue::as_u64),
                    Some(2),
                    "second transaction must see the first commit"
                );
                assert_eq!(
                    tasks.get("first").and_then(shared::JsonValue::as_bool),
                    Some(true)
                );
                Ok(shared::JsonValue::Null)
            })
            .expect("second transaction");
        writer.join().expect("writer");
    }

    #[test]
    fn backup_creates_missing_parent_directory() {
        let dir = std::path::PathBuf::from("target/test-tmp/store-backup-parent");
        let _ = std::fs::remove_dir_all(&dir);
        let store = SqliteStore::open(&dir.join("me.db")).unwrap();
        let mut doc = shared::Document::new();
        doc.insert("version".into(), shared::JsonValue::from(1_u64));
        store.insert_initial("tasks", &doc).unwrap();

        let target = dir.join("nested/backups/me.db");
        store.backup_to(&target).expect("backup creates parent");
        assert!(target.is_file());
        let restored = SqliteStore::open(&target).unwrap();
        assert_eq!(
            restored
                .load("tasks")
                .unwrap()
                .get("version")
                .and_then(shared::JsonValue::as_u64),
            Some(1)
        );
    }

    #[test]
    fn corrupt_body_reports_data_invalid() {
        let dir = std::path::PathBuf::from("target/test-tmp/store-corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("me.db");
        let store = SqliteStore::open(&path).unwrap();
        let mut doc = shared::Document::new();
        doc.insert("version".into(), shared::JsonValue::from(1_u64));
        store.insert_initial("tasks", &doc).unwrap();

        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE documents SET body = 'not json' WHERE collection = 'tasks'",
                [],
            )
            .unwrap();

        let error = store.load("tasks").expect_err("corrupt");
        assert_eq!(error.code, "DATA_INVALID");
    }

    #[test]
    fn busy_database_reports_write_busy() {
        let dir = std::path::PathBuf::from("target/test-tmp/store-busy");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("me.db");
        let store = SqliteStore::open(&path).unwrap();
        let mut doc = shared::Document::new();
        doc.insert("version".into(), shared::JsonValue::from(1_u64));
        store.insert_initial("tasks", &doc).unwrap();

        let blocker = rusqlite::Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let request = Request {
            request_id: "req-busy".into(),
            expected_version: 1,
            input: Default::default(),
        };
        let error = store
            .with_document_mut("tasks", &request, "task.create", &mut |_| {
                Ok(shared::JsonValue::Object(Default::default()))
            })
            .expect_err("busy");
        assert_eq!(error.code, "WRITE_BUSY");
        let _ = blocker.execute_batch("ROLLBACK");
    }

    #[test]
    fn sqlite_creates_missing_collection_only_with_expected_version_zero() {
        let store = SqliteStore::open_in_memory().unwrap();
        let stale = Request {
            request_id: "r1".into(),
            expected_version: 1,
            input: Default::default(),
        };
        assert_eq!(
            store
                .with_document_mut("calendar", &stale, "migrate", &mut |_| {
                    Ok(shared::JsonValue::Object(Default::default()))
                })
                .unwrap_err()
                .code,
            "NOT_FOUND"
        );

        let fresh = Request {
            request_id: "r2".into(),
            expected_version: 0,
            input: Default::default(),
        };
        store
            .with_document_mut("calendar", &fresh, "migrate", &mut |doc| {
                doc.insert("items".into(), shared::JsonValue::Array(Vec::new()));
                Ok(shared::JsonValue::Object(Default::default()))
            })
            .unwrap();
        assert_eq!(
            store
                .load("calendar")
                .unwrap()
                .get("version")
                .and_then(shared::JsonValue::as_u64),
            Some(1)
        );
    }

    #[test]
    fn backup_creates_openable_snapshot_and_refuses_existing_target() {
        let dir = std::path::PathBuf::from("target/test-tmp/store-backup");
        let _ = std::fs::remove_dir_all(&dir);
        let store = SqliteStore::open(&dir.join("me.db")).unwrap();
        let mut doc = shared::Document::new();
        doc.insert("version".into(), shared::JsonValue::from(3_u64));
        store.insert_initial("tasks", &doc).unwrap();
        let target = dir.join("backup.db");
        store.backup_to(&target).unwrap();
        assert!(store.backup_to(&target).is_err());
        let restored = SqliteStore::open(&target).unwrap();
        assert_eq!(
            restored
                .load("tasks")
                .unwrap()
                .get("version")
                .and_then(shared::JsonValue::as_u64),
            Some(3)
        );
    }
}
