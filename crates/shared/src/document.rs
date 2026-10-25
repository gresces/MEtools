//! Storage-agnostic document model and the transactional mutation algorithm.
//!
//! Domain handlers depend on [`DocumentStore`] instead of a concrete database so
//! business rules can run against SQLite in production and an in-memory backend
//! in tests.

use std::collections::BTreeMap;

use crate::json::JsonValue;
use crate::storage::{now_iso8601, remember_request, replay_request};
use crate::{FunctionFailure, Request};

// ---- Document ---------------------------------------------------------------

/// One named collection, stored as the same JSON document shape the domain
/// handlers already validate and mutate.
pub type Document = BTreeMap<String, JsonValue>;

// ---- MutateOutcome ----------------------------------------------------------

/// Result of a transactional mutation: either a fresh write or an idempotent replay.
#[derive(Debug, Clone)]
pub enum MutateOutcome {
    Applied { result: JsonValue, version: u64 },
    Replayed(JsonValue),
}

// ---- DocumentStore ----------------------------------------------------------

/// Transaction-scoped view used when a handler must read other collections
/// under the same write lock as the mutation it performs.
pub trait DocumentTransaction {
    fn load(&mut self, collection: &str) -> Result<Document, FunctionFailure>;
    fn load_optional(&mut self, collection: &str) -> Result<Option<Document>, FunctionFailure>;
    fn mutate(
        &mut self,
        collection: &str,
        request: &Request,
        operation: &str,
        mutate: &mut dyn FnMut(&mut Document) -> Result<JsonValue, FunctionFailure>,
    ) -> Result<MutateOutcome, FunctionFailure>;
    fn insert_initial(
        &mut self,
        collection: &str,
        document: &Document,
    ) -> Result<bool, FunctionFailure>;
    fn replace(&mut self, collection: &str, document: &Document) -> Result<(), FunctionFailure>;
    /// Remove a collection. Returns `true` when it existed.
    fn remove(&mut self, collection: &str) -> Result<bool, FunctionFailure>;
}

/// Persistence port used by domain handlers. Implementations own transaction
/// semantics; callers only see documents and outcomes.
pub trait DocumentStore: Send + Sync {
    fn collections(&self) -> Result<Vec<String>, FunctionFailure>;
    fn load(&self, collection: &str) -> Result<Document, FunctionFailure>;
    fn load_optional(&self, collection: &str) -> Result<Option<Document>, FunctionFailure>;
    /// Read just the collection version for conditional requests. Implementations
    /// should avoid materializing the whole document when possible.
    fn version(&self, collection: &str) -> Result<Option<u64>, FunctionFailure> {
        Ok(self
            .load_optional(collection)?
            .and_then(|document| document.get("version").and_then(JsonValue::as_u64)))
    }
    fn insert_initial(
        &self,
        collection: &str,
        document: &Document,
    ) -> Result<bool, FunctionFailure>;
    fn replace(&self, collection: &str, document: &Document) -> Result<(), FunctionFailure>;
    /// Remove a collection. Returns `true` when it existed.
    fn remove(&self, collection: &str) -> Result<bool, FunctionFailure>;
    /// Run `body` inside one write transaction. Reads through the transaction
    /// see the same snapshot as the writes, so cross-collection invariants hold.
    fn transaction(
        &self,
        body: &mut dyn FnMut(&mut dyn DocumentTransaction) -> Result<JsonValue, FunctionFailure>,
    ) -> Result<JsonValue, FunctionFailure>;

    fn with_document_mut(
        &self,
        collection: &str,
        request: &Request,
        operation: &str,
        mutate: &mut dyn FnMut(&mut Document) -> Result<JsonValue, FunctionFailure>,
    ) -> Result<MutateOutcome, FunctionFailure> {
        let mut captured = None;
        self.transaction(&mut |transaction| {
            captured = Some(transaction.mutate(collection, request, operation, mutate)?);
            Ok(JsonValue::Null)
        })?;
        captured.ok_or_else(|| {
            FunctionFailure::new("INTERNAL_ERROR", "transaction did not run the mutation")
        })
    }
}

// ---- apply_mutation ---------------------------------------------------------

/// Apply one idempotent mutation to an already loaded document.
///
/// Replay is checked first, so retrying a request whose write already committed
/// still returns the original result. `expected_version` semantics are
/// domain-scoped (collection or record), so handlers perform that check inside
/// `mutate`. The document is only replaced on success, which gives
/// implementations a rollback story without cloning inside the closure.
pub fn apply_mutation(
    document: &mut Document,
    request: &Request,
    operation: &str,
    mutate: &mut dyn FnMut(&mut Document) -> Result<JsonValue, FunctionFailure>,
) -> Result<MutateOutcome, FunctionFailure> {
    if let Some(replayed) = replay_request(document, request, operation)? {
        return Ok(MutateOutcome::Replayed(replayed));
    }

    let current_version = document
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let mut working = document.clone();
    let mut result = mutate(&mut working)?;
    let next_version = current_version + 1;
    working.insert("version".to_string(), JsonValue::from(next_version));
    working.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
    working.insert("updated_by".to_string(), JsonValue::from(operation));
    if let JsonValue::Object(map) = &mut result {
        map.insert(
            "collection_version".to_string(),
            JsonValue::from(next_version),
        );
    }
    remember_request(&mut working, request, operation, &result);

    *document = working;
    Ok(MutateOutcome::Applied {
        result,
        version: next_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Request;
    use crate::json::JsonValue;
    use std::collections::BTreeMap;

    fn request(id: &str, expected: u64) -> Request {
        Request {
            request_id: id.into(),
            expected_version: expected,
            input: BTreeMap::new(),
        }
    }

    fn empty_result() -> JsonValue {
        JsonValue::Object(BTreeMap::new())
    }

    fn document() -> Document {
        let mut doc = Document::new();
        doc.insert("version".into(), JsonValue::from(1_u64));
        doc.insert(
            "updated_at".into(),
            JsonValue::from("2026-10-22T00:00:00+08:00"),
        );
        doc.insert("updated_by".into(), JsonValue::from("init"));
        doc.insert("items".into(), JsonValue::Array(Vec::new()));
        doc
    }

    #[test]
    fn applied_mutation_bumps_version_and_injects_collection_version() {
        let mut doc = document();
        let outcome = apply_mutation(&mut doc, &request("req-1", 1), "task.create", &mut |doc| {
            doc.insert("title".into(), JsonValue::from("write plan"));
            Ok(JsonValue::object([(
                "status",
                JsonValue::from("succeeded"),
            )]))
        })
        .expect("apply");
        let MutateOutcome::Applied { result, version } = outcome else {
            panic!("expected applied")
        };
        assert_eq!(version, 2);
        assert_eq!(
            result.get("collection_version").and_then(JsonValue::as_u64),
            Some(2)
        );
        assert_eq!(doc.get("version").and_then(JsonValue::as_u64), Some(2));
        assert_eq!(
            doc.get("updated_by").and_then(JsonValue::as_str),
            Some("task.create")
        );
    }

    #[test]
    fn replayed_request_skips_version_check_and_returns_saved_result() {
        let mut doc = document();
        apply_mutation(&mut doc, &request("req-3", 1), "task.create", &mut |_| {
            Ok(JsonValue::object([("tag", JsonValue::from("first"))]))
        })
        .expect("first");
        let outcome = apply_mutation(&mut doc, &request("req-3", 1), "task.create", &mut |_| {
            panic!("mutate must not run for a replay")
        })
        .expect("replay");
        let MutateOutcome::Replayed(value) = outcome else {
            panic!("expected replay")
        };
        assert_eq!(value.get("tag").and_then(JsonValue::as_str), Some("first"));
        assert_eq!(
            value.get("collection_version").and_then(JsonValue::as_u64),
            Some(2)
        );
        assert_eq!(doc.get("version").and_then(JsonValue::as_u64), Some(2));
    }

    #[test]
    fn request_id_reuse_with_different_input_is_rejected() {
        let mut doc = document();
        apply_mutation(&mut doc, &request("req-4", 1), "task.create", &mut |_| {
            Ok(empty_result())
        })
        .expect("first");
        let mut changed = request("req-4", 2);
        changed
            .input
            .insert("title".into(), JsonValue::from("different"));
        let error = apply_mutation(&mut doc, &changed, "task.create", &mut |_| {
            Ok(empty_result())
        })
        .expect_err("conflict");
        assert_eq!(error.code, "REQUEST_ID_CONFLICT");
    }

    #[test]
    fn receipts_are_capped_at_the_configured_limit() {
        let mut doc = document();
        for index in 0..(crate::storage::RECEIPT_LIMIT + 2) {
            let expected = doc.get("version").and_then(JsonValue::as_u64).unwrap_or(0);
            apply_mutation(
                &mut doc,
                &request(&format!("req-{index}"), expected),
                "task.create",
                &mut |_| Ok(empty_result()),
            )
            .expect("apply");
        }
        let receipts = doc
            .get("recent_requests")
            .and_then(JsonValue::as_array)
            .expect("receipts");
        assert_eq!(receipts.len(), crate::storage::RECEIPT_LIMIT);
    }

    #[test]
    fn non_object_results_do_not_gain_a_collection_version() {
        let mut doc = document();
        let outcome = apply_mutation(&mut doc, &request("req-scalar", 1), "op", &mut |_| {
            Ok(JsonValue::from("scalar"))
        })
        .expect("apply");
        let MutateOutcome::Applied { result, version } = outcome else {
            panic!("expected applied")
        };
        assert_eq!(version, 2);
        assert!(result.as_object().is_none());
    }

    #[test]
    fn failed_mutation_leaves_document_unchanged() {
        let mut doc = document();
        let before = doc.clone();
        let error = apply_mutation(&mut doc, &request("req-5", 1), "task.create", &mut |doc| {
            doc.insert("title".into(), JsonValue::from("half written"));
            Err(crate::FunctionFailure::invalid_input("boom"))
        })
        .expect_err("failure");
        assert_eq!(error.code, "INVALID_INPUT");
        assert_eq!(doc, before);
    }
}
