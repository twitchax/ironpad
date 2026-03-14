//! Validation for imported notebook JSON.
//!
//! Ensures the JSON structure matches the expected `IronpadNotebook` shape
//! before passing it to `importNotebook` in storage.js.

/// Validates that a JSON string contains a valid notebook structure.
///
/// Checks for required top-level fields (`version`, `title`, `cells`) and
/// required per-cell fields (`id`, `source`, `label`).
pub fn validate_notebook_json(json: &str) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Invalid JSON: {e}"))?;

    let obj = value
        .as_object()
        .ok_or_else(|| "Expected a JSON object".to_string())?;

    if !obj.get("version").is_some_and(serde_json::Value::is_number) {
        return Err("Missing or invalid 'version' field (expected a number)".to_string());
    }

    if !obj.get("title").is_some_and(serde_json::Value::is_string) {
        return Err("Missing or invalid 'title' field (expected a string)".to_string());
    }

    let cells = obj
        .get("cells")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Missing or invalid 'cells' field (expected an array)".to_string())?;

    for (i, cell) in cells.iter().enumerate() {
        let cell_obj = cell
            .as_object()
            .ok_or_else(|| format!("Cell {i} is not a JSON object"))?;

        if !cell_obj.get("id").is_some_and(serde_json::Value::is_string) {
            return Err(format!("Cell {i}: missing or invalid 'id' field"));
        }
        if !cell_obj
            .get("source")
            .is_some_and(serde_json::Value::is_string)
        {
            return Err(format!("Cell {i}: missing or invalid 'source' field"));
        }
        if !cell_obj
            .get("label")
            .is_some_and(serde_json::Value::is_string)
        {
            return Err(format!("Cell {i}: missing or invalid 'label' field"));
        }
    }

    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_notebook_with_cells() {
        let json = r#"{
            "version": 1,
            "title": "Test Notebook",
            "cells": [
                {"id": "c1", "source": "42", "label": "Cell 1"},
                {"id": "c2", "source": "println!(\"hello\")", "label": "Cell 2"}
            ]
        }"#;
        assert!(validate_notebook_json(json).is_ok());
    }

    #[test]
    fn valid_notebook_empty_cells() {
        let json = r#"{"version": 1, "title": "Empty", "cells": []}"#;
        assert!(validate_notebook_json(json).is_ok());
    }

    #[test]
    fn invalid_json_syntax() {
        let result = validate_notebook_json("not json at all");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid JSON"));
    }

    #[test]
    fn missing_version() {
        let json = r#"{"title": "Test", "cells": []}"#;
        let result = validate_notebook_json(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("version"));
    }

    #[test]
    fn version_wrong_type() {
        let json = r#"{"version": "one", "title": "Test", "cells": []}"#;
        let result = validate_notebook_json(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("version"));
    }

    #[test]
    fn missing_title() {
        let json = r#"{"version": 1, "cells": []}"#;
        let result = validate_notebook_json(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("title"));
    }

    #[test]
    fn missing_cells() {
        let json = r#"{"version": 1, "title": "Test"}"#;
        let result = validate_notebook_json(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cells"));
    }

    #[test]
    fn cells_not_array() {
        let json = r#"{"version": 1, "title": "Test", "cells": "not an array"}"#;
        let result = validate_notebook_json(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cells"));
    }

    #[test]
    fn cell_missing_id() {
        let json = r#"{"version": 1, "title": "Test", "cells": [{"source": "42", "label": "C1"}]}"#;
        let result = validate_notebook_json(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Cell 0"));
        assert!(err.contains("id"));
    }

    #[test]
    fn cell_missing_source() {
        let json = r#"{"version": 1, "title": "Test", "cells": [{"id": "c1", "label": "C1"}]}"#;
        let result = validate_notebook_json(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("source"));
    }

    #[test]
    fn cell_missing_label() {
        let json = r#"{"version": 1, "title": "Test", "cells": [{"id": "c1", "source": "42"}]}"#;
        let result = validate_notebook_json(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("label"));
    }

    #[test]
    fn not_an_object() {
        let json = r"[1, 2, 3]";
        let result = validate_notebook_json(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("object"));
    }
}
