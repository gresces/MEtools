//! In-memory [`DocumentStore`] for tests and local experiments.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use shared::{
    Document, DocumentStore, DocumentTransaction, FunctionFailure, JsonValue, MutateOutcome,
    Request, apply_mutation,
};

/// Holds every collection in memory behind a mutex.
pub struct MemoryStore {
    documents: Mutex<BTreeMap<String, Document>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            documents: Mutex::new(BTreeMap::new()),
        }
    }

    fn guard(&self) -> Result<MutexGuard<'_, BTreeMap<String, Document>>, FunctionFailure> {
        self.documents
            .lock()
            .map_err(|_| FunctionFailure::new("INTERNAL_ERROR", "memory store lock is poisoned"))
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentStore for MemoryStore {
    fn collections(&self) -> Result<Vec<String>, FunctionFailure> {
        Ok(self.guard()?.keys().cloned().collect())
    }

    fn load(&self, collection: &str) -> Result<Document, FunctionFailure> {
        self.load_optional(collection)?.ok_or_else(|| {
            FunctionFailure::not_found(format!("collection {collection} does not exist"))
        })
    }

    fn load_optional(&self, collection: &str) -> Result<Option<Document>, FunctionFailure> {
        Ok(self.guard()?.get(collection).cloned())
    }

    fn version(&self, collection: &str) -> Result<Option<u64>, FunctionFailure> {
        Ok(self
            .guard()?
            .get(collection)
            .and_then(|document| document.get("version").and_then(shared::JsonValue::as_u64)))
    }

    fn insert_initial(
        &self,
        collection: &str,
        document: &Document,
    ) -> Result<bool, FunctionFailure> {
        let mut documents = self.guard()?;
        if documents.contains_key(collection) {
            return Ok(false);
        }
        documents.insert(collection.to_string(), document.clone());
        Ok(true)
    }

    fn replace(&self, collection: &str, document: &Document) -> Result<(), FunctionFailure> {
        self.guard()?
            .insert(collection.to_string(), document.clone());
        Ok(())
    }

    fn remove(&self, collection: &str) -> Result<bool, FunctionFailure> {
        Ok(self.guard()?.remove(collection).is_some())
    }

    fn transaction(
        &self,
        body: &mut dyn FnMut(&mut dyn DocumentTransaction) -> Result<JsonValue, FunctionFailure>,
    ) -> Result<JsonValue, FunctionFailure> {
        let mut documents = self.guard()?;
        let snapshot = documents.clone();
        let result = {
            let mut transaction = MemoryTransaction {
                documents: &mut documents,
            };
            body(&mut transaction)
        };
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                *documents = snapshot;
                Err(error)
            }
        }
    }
}

/// Transaction view over the in-memory map; the caller restores the snapshot on error.
struct MemoryTransaction<'a> {
    documents: &'a mut BTreeMap<String, Document>,
}

impl DocumentTransaction for MemoryTransaction<'_> {
    fn load(&mut self, collection: &str) -> Result<Document, FunctionFailure> {
        self.load_optional(collection)?.ok_or_else(|| {
            FunctionFailure::not_found(format!("collection {collection} does not exist"))
        })
    }

    fn load_optional(&mut self, collection: &str) -> Result<Option<Document>, FunctionFailure> {
        Ok(self.documents.get(collection).cloned())
    }

    fn mutate(
        &mut self,
        collection: &str,
        request: &Request,
        operation: &str,
        mutate: &mut dyn FnMut(&mut Document) -> Result<JsonValue, FunctionFailure>,
    ) -> Result<MutateOutcome, FunctionFailure> {
        let mut document = match self.documents.get(collection) {
            Some(document) => document.clone(),
            None if request.expected_version == 0 => Document::new(),
            None => {
                return Err(FunctionFailure::not_found(format!(
                    "collection {collection} does not exist"
                )));
            }
        };

        let outcome = apply_mutation(&mut document, request, operation, mutate)?;
        if matches!(outcome, MutateOutcome::Applied { .. }) {
            self.documents.insert(collection.to_string(), document);
        }
        Ok(outcome)
    }

    fn insert_initial(
        &mut self,
        collection: &str,
        document: &Document,
    ) -> Result<bool, FunctionFailure> {
        if self.documents.contains_key(collection) {
            return Ok(false);
        }
        self.documents
            .insert(collection.to_string(), document.clone());
        Ok(true)
    }

    fn replace(&mut self, collection: &str, document: &Document) -> Result<(), FunctionFailure> {
        self.documents
            .insert(collection.to_string(), document.clone());
        Ok(())
    }

    fn remove(&mut self, collection: &str) -> Result<bool, FunctionFailure> {
        Ok(self.documents.remove(collection).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryStore;
    use shared::{Document, DocumentStore, JsonValue, MutateOutcome, Request};

    #[test]
    fn insert_initial_is_idempotent_and_load_returns_document() {
        let store = MemoryStore::new();
        assert!(store.collections().unwrap().is_empty());
        let mut doc = Document::new();
        doc.insert("version".into(), JsonValue::from(1_u64));
        assert!(store.insert_initial("tasks", &doc).unwrap());
        assert!(!store.insert_initial("tasks", &doc).unwrap());
        assert_eq!(store.load("tasks").unwrap(), doc);
        assert_eq!(store.collections().unwrap(), vec!["tasks".to_string()]);
    }

    #[test]
    fn load_missing_collection_reports_not_found() {
        let store = MemoryStore::new();
        assert_eq!(store.load("tasks").unwrap_err().code, "NOT_FOUND");
        assert!(store.load_optional("tasks").unwrap().is_none());
    }

    #[test]
    fn transaction_rolls_back_every_collection_on_error() {
        let store = MemoryStore::new();
        let mut doc = Document::new();
        doc.insert("version".into(), JsonValue::from(1_u64));
        store.insert_initial("tasks", &doc).unwrap();
        store.insert_initial("calendar", &doc).unwrap();

        let error = store
            .transaction(&mut |transaction| {
                transaction.mutate(
                    "tasks",
                    &Request {
                        request_id: "r1".into(),
                        expected_version: 1,
                        input: Default::default(),
                    },
                    "task.create",
                    &mut |document| {
                        document.insert("title".into(), JsonValue::from("half"));
                        Ok(JsonValue::Object(Default::default()))
                    },
                )?;
                transaction.replace("calendar", &Document::new())?;
                Err(shared::FunctionFailure::invalid_input("boom"))
            })
            .expect_err("rollback");
        assert_eq!(error.code, "INVALID_INPUT");
        assert_eq!(store.load("tasks").unwrap(), doc);
        assert_eq!(store.load("calendar").unwrap(), doc);
    }

    #[test]
    fn missing_collection_is_created_only_with_expected_version_zero() {
        let store = MemoryStore::new();
        let stale = Request {
            request_id: "r1".into(),
            expected_version: 1,
            input: Default::default(),
        };
        let empty = || Ok(JsonValue::Object(Default::default()));
        assert_eq!(
            store
                .with_document_mut("calendar", &stale, "migrate", &mut |_| empty())
                .unwrap_err()
                .code,
            "NOT_FOUND"
        );

        let fresh = Request {
            request_id: "r2".into(),
            expected_version: 0,
            input: Default::default(),
        };
        let outcome = store
            .with_document_mut("calendar", &fresh, "migrate", &mut |doc| {
                doc.insert("items".into(), JsonValue::Array(Vec::new()));
                empty()
            })
            .unwrap();
        assert!(matches!(outcome, MutateOutcome::Applied { version: 1, .. }));
        assert_eq!(
            store
                .load("calendar")
                .unwrap()
                .get("version")
                .and_then(JsonValue::as_u64),
            Some(1)
        );
    }
}
