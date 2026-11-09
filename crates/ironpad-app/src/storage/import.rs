//! Decoding an imported `.ironpad` file.
//!
//! `IronpadNotebook`'s serde shape is the ONLY schema: the file is decoded
//! through it, and the caller writes nothing to `IndexedDB` unless it decoded.
//! A hand-written validator used to mirror that shape, and it drifted (it never
//! required a cell's `order`, and took any number as a `version`), so a file it
//! passed was saved first and failed to decode second, stranding a record the
//! home page skips on every load and can never list or delete.

use ironpad_common::IronpadNotebook;

/// Decode `json` as a fresh local copy: a new id, and both timestamps set to
/// now, so the import can never collide with the notebook it was exported
/// from. Those three fields are overwritten BEFORE decoding, so a hand-written
/// file may leave them out; everything else must satisfy `IronpadNotebook`,
/// and the error names what did not.
pub fn notebook_from_import_json(json: &str) -> Result<IronpadNotebook, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Invalid JSON: {e}"))?;
    let fields = value
        .as_object_mut()
        .ok_or_else(|| "Expected a JSON object".to_string())?;

    let now = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
    fields.insert(
        "id".to_string(),
        serde_json::Value::String(uuid::Uuid::new_v4().to_string()),
    );
    fields.insert("created_at".to_string(), now.clone());
    fields.insert("updated_at".to_string(), now);

    serde_json::from_value(value).map_err(|e| format!("Not an ironpad notebook: {e}"))
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rejection(json: &str) -> String {
        notebook_from_import_json(json).expect_err("should have been rejected")
    }

    #[test]
    fn a_minimal_notebook_imports_with_a_fresh_identity() {
        let json = r#"{
            "version": 1,
            "id": "00000000-0000-0000-0000-000000000001",
            "title": "Test Notebook",
            "created_at": "2020-01-01T00:00:00Z",
            "updated_at": "2020-01-01T00:00:00Z",
            "cells": [
                {"id": "c1", "order": 0, "source": "42", "label": "Cell 1"},
                {"id": "c2", "order": 1, "source": "println!(\"hello\")", "label": "Cell 2"}
            ]
        }"#;
        let nb = notebook_from_import_json(json).expect("a valid notebook");
        assert_eq!(nb.title, "Test Notebook");
        assert_eq!(nb.cells.len(), 2);
        assert_ne!(
            nb.id.to_string(),
            "00000000-0000-0000-0000-000000000001",
            "an import must never reuse the exported notebook's id"
        );
        assert!(nb.created_at.timestamp() > 1_700_000_000);
        assert_eq!(nb.created_at, nb.updated_at);
    }

    #[test]
    fn identity_and_timestamps_may_be_left_out() {
        let nb = notebook_from_import_json(r#"{"version": 1, "title": "Empty", "cells": []}"#)
            .expect("a hand-written file needs no id or timestamps");
        assert!(nb.cells.is_empty());
    }

    #[test]
    fn invalid_json_syntax() {
        assert!(rejection("not json at all").contains("Invalid JSON"));
    }

    #[test]
    fn not_an_object() {
        assert!(rejection("[1, 2, 3]").contains("object"));
    }

    #[test]
    fn missing_version() {
        assert!(rejection(r#"{"title": "Test", "cells": []}"#).contains("version"));
    }

    #[test]
    fn version_wrong_type() {
        assert!(rejection(r#"{"version": "one", "title": "Test", "cells": []}"#).contains("u32"));
    }

    #[test]
    fn missing_title() {
        assert!(rejection(r#"{"version": 1, "cells": []}"#).contains("title"));
    }

    #[test]
    fn missing_cells() {
        assert!(rejection(r#"{"version": 1, "title": "Test"}"#).contains("cells"));
    }

    #[test]
    fn cells_not_array() {
        assert!(
            rejection(r#"{"version": 1, "title": "Test", "cells": "not an array"}"#)
                .contains("sequence")
        );
    }

    #[test]
    fn cell_missing_id() {
        let json = r#"{"version": 1, "title": "T", "cells": [{"order": 0, "source": "42", "label": "C1"}]}"#;
        assert!(rejection(json).contains("`id`"));
    }

    #[test]
    fn cell_missing_source() {
        let json =
            r#"{"version": 1, "title": "T", "cells": [{"id": "c1", "order": 0, "label": "C1"}]}"#;
        assert!(rejection(json).contains("`source`"));
    }

    #[test]
    fn cell_missing_label() {
        let json =
            r#"{"version": 1, "title": "T", "cells": [{"id": "c1", "order": 0, "source": "42"}]}"#;
        assert!(rejection(json).contains("`label`"));
    }

    #[test]
    fn import_rejects_what_the_type_rejects() {
        // Both passed the hand-written validator, then failed to decode AFTER
        // they had been written to IndexedDB.
        let no_order = r#"{"version": 1, "title": "T", "cells": [{"id": "c1", "source": "42", "label": "C1"}]}"#;
        assert!(rejection(no_order).contains("`order`"));

        let fractional_version = r#"{"version": 1.5, "title": "T", "cells": []}"#;
        assert!(rejection(fractional_version).contains("u32"));
    }

    #[test]
    fn import_accepts_a_real_public_notebook() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../public/notebooks/welcome.ironpad"
        );
        let json = std::fs::read_to_string(path).expect("welcome.ironpad is readable");
        let nb = notebook_from_import_json(&json).expect("a shipped notebook imports");
        assert_eq!(nb.title, "Welcome to ironpad");
        assert!(!nb.cells.is_empty());
    }
}
