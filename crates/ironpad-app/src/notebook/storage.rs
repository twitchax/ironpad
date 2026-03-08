//! Notebook filesystem CRUD operations.
//!
//! Directory layout under `{data_dir}/notebooks/{id}/`:
//! ```text
//! {id}/
//!   ironpad.json          # notebook manifest
//!   cells/                # cell subdirectories (managed by T-015)
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use ironpad_common::{CellManifest, NotebookManifest, NotebookSummary};
use uuid::Uuid;

// ── Path helpers ─────────────────────────────────────────────────────────────

/// Returns the top-level notebooks directory.
pub fn notebooks_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("notebooks")
}

/// Returns the directory for a specific notebook.
pub fn notebook_dir(data_dir: &Path, id: &Uuid) -> PathBuf {
    notebooks_dir(data_dir).join(id.to_string())
}

/// Returns the path to a notebook's manifest file.
pub fn manifest_path(data_dir: &Path, id: &Uuid) -> PathBuf {
    notebook_dir(data_dir, id).join("ironpad.json")
}

// ── CRUD operations ──────────────────────────────────────────────────────────

/// Creates a new notebook with the given title.
///
/// Generates a UUID, writes `ironpad.json`, and creates the `cells/` directory.
/// Returns the new manifest.
pub fn create_notebook(data_dir: &Path, title: &str) -> Result<NotebookManifest> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    let manifest = NotebookManifest {
        id,
        title: title.to_string(),
        created_at: now,
        updated_at: now,
        compiler_version: "stable".to_string(),
        cells: vec![],
        shared_cargo_toml: None,
    };

    let nb_dir = notebook_dir(data_dir, &id);
    std::fs::create_dir_all(nb_dir.join("cells"))
        .with_context(|| format!("Failed to create notebook directory: {}", nb_dir.display()))?;

    write_manifest(data_dir, &manifest)?;

    tracing::info!(id = %id, title = %title, "created notebook");
    Ok(manifest)
}

/// Reads a notebook manifest from disk.
pub fn get_notebook(data_dir: &Path, id: &Uuid) -> Result<NotebookManifest> {
    let path = manifest_path(data_dir, id);

    let json = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read notebook manifest: {}", path.display()))?;

    let manifest: NotebookManifest = serde_json::from_str(&json)
        .with_context(|| format!("Failed to parse notebook manifest: {}", path.display()))?;

    Ok(manifest)
}

/// Updates a notebook manifest on disk.
///
/// Optionally updates the title and/or cell list. Always bumps `updated_at`.
pub fn update_notebook(
    data_dir: &Path,
    id: &Uuid,
    title: Option<&str>,
    cells: Option<Vec<CellManifest>>,
) -> Result<NotebookManifest> {
    let mut manifest = get_notebook(data_dir, id)?;

    if let Some(t) = title {
        manifest.title = t.to_string();
    }
    if let Some(c) = cells {
        manifest.cells = c;
    }

    manifest.updated_at = Utc::now();
    write_manifest(data_dir, &manifest)?;

    tracing::info!(id = %id, "updated notebook");
    Ok(manifest)
}

/// Deletes an entire notebook directory (manifest + all cells).
pub fn delete_notebook(data_dir: &Path, id: &Uuid) -> Result<()> {
    let dir = notebook_dir(data_dir, id);

    std::fs::remove_dir_all(&dir)
        .with_context(|| format!("Failed to delete notebook: {}", dir.display()))?;

    tracing::info!(id = %id, "deleted notebook");
    Ok(())
}

/// Lists all notebooks, returning lightweight summaries sorted by `updated_at` (newest first).
pub fn list_notebooks(data_dir: &Path) -> Result<Vec<NotebookSummary>> {
    let dir = notebooks_dir(data_dir);

    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut summaries = Vec::new();

    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to read notebooks directory: {}", dir.display()))?
    {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }

        let mpath = entry.path().join("ironpad.json");
        if !mpath.exists() {
            continue;
        }

        match read_manifest(&mpath) {
            Ok(manifest) => {
                summaries.push(NotebookSummary {
                    id: manifest.id,
                    title: manifest.title,
                    updated_at: manifest.updated_at,
                    cell_count: manifest.cells.len(),
                });
            }
            Err(e) => {
                tracing::warn!(
                    path = %mpath.display(),
                    error = %e,
                    "skipping malformed notebook"
                );
            }
        }
    }

    summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(summaries)
}

// ── Shared Cargo.toml ────────────────────────────────────────────────────────

/// Returns the path to a notebook's shared `Cargo.toml` file.
pub fn shared_cargo_toml_path(data_dir: &Path, id: &Uuid) -> PathBuf {
    notebook_dir(data_dir, id).join("Cargo.toml")
}

/// Reads the notebook-level shared `Cargo.toml`, if it exists.
pub fn get_shared_cargo_toml(data_dir: &Path, id: &Uuid) -> Result<Option<String>> {
    let path = shared_cargo_toml_path(data_dir, id);
    if path.exists() {
        Ok(Some(std::fs::read_to_string(&path).with_context(|| {
            format!("Failed to read shared Cargo.toml: {}", path.display())
        })?))
    } else {
        Ok(None)
    }
}

/// Writes or overwrites the notebook-level shared `Cargo.toml`.
pub fn update_shared_cargo_toml(data_dir: &Path, id: &Uuid, content: &str) -> Result<()> {
    let path = shared_cargo_toml_path(data_dir, id);
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write shared Cargo.toml: {}", path.display()))?;
    Ok(())
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Writes a manifest to disk as pretty-printed JSON.
fn write_manifest(data_dir: &Path, manifest: &NotebookManifest) -> Result<()> {
    let path = manifest_path(data_dir, &manifest.id);

    let json =
        serde_json::to_string_pretty(manifest).context("Failed to serialize notebook manifest")?;

    std::fs::write(&path, json)
        .with_context(|| format!("Failed to write notebook manifest: {}", path.display()))?;

    Ok(())
}

/// Reads and parses a manifest from a given path.
fn read_manifest(path: &Path) -> Result<NotebookManifest> {
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read: {}", path.display()))?;

    serde_json::from_str(&json).with_context(|| format!("Failed to parse: {}", path.display()))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ironpad_common::CellType;

    fn temp_data_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    #[test]
    fn create_notebook_writes_manifest_and_cells_dir() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let manifest = create_notebook(data_dir, "Test Notebook").unwrap();

        assert_eq!(manifest.title, "Test Notebook");
        assert_eq!(manifest.compiler_version, "stable");
        assert!(manifest.cells.is_empty());

        // Verify directory structure.
        let nb_dir = notebook_dir(data_dir, &manifest.id);
        assert!(nb_dir.join("ironpad.json").exists());
        assert!(nb_dir.join("cells").is_dir());
    }

    #[test]
    fn get_notebook_reads_manifest() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let created = create_notebook(data_dir, "Readable").unwrap();
        let loaded = get_notebook(data_dir, &created.id).unwrap();

        assert_eq!(created.id, loaded.id);
        assert_eq!(created.title, loaded.title);
        assert_eq!(created.compiler_version, loaded.compiler_version);
    }

    #[test]
    fn get_notebook_missing_returns_error() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let fake_id = Uuid::new_v4();

        assert!(get_notebook(data_dir, &fake_id).is_err());
    }

    #[test]
    fn update_notebook_title() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let created = create_notebook(data_dir, "Original").unwrap();
        let updated = update_notebook(data_dir, &created.id, Some("Renamed"), None).unwrap();

        assert_eq!(updated.title, "Renamed");
        assert!(updated.updated_at >= created.updated_at);

        // Verify persisted.
        let reloaded = get_notebook(data_dir, &created.id).unwrap();
        assert_eq!(reloaded.title, "Renamed");
    }

    #[test]
    fn update_notebook_cells() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let created = create_notebook(data_dir, "Cells Test").unwrap();
        let cells = vec![
            CellManifest {
                id: "cell_0".to_string(),
                order: 0,
                label: "First".to_string(),
                cell_type: CellType::default(),
            },
            CellManifest {
                id: "cell_1".to_string(),
                order: 1,
                label: "Second".to_string(),
                cell_type: CellType::default(),
            },
        ];

        let updated = update_notebook(data_dir, &created.id, None, Some(cells.clone())).unwrap();

        assert_eq!(updated.cells.len(), 2);
        assert_eq!(updated.cells[0].id, "cell_0");
        assert_eq!(updated.cells[1].label, "Second");
    }

    #[test]
    fn delete_notebook_removes_directory() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let created = create_notebook(data_dir, "Doomed").unwrap();
        let nb_dir = notebook_dir(data_dir, &created.id);
        assert!(nb_dir.exists());

        delete_notebook(data_dir, &created.id).unwrap();
        assert!(!nb_dir.exists());
    }

    #[test]
    fn delete_notebook_missing_returns_error() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let fake_id = Uuid::new_v4();

        assert!(delete_notebook(data_dir, &fake_id).is_err());
    }

    #[test]
    fn list_notebooks_empty() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let summaries = list_notebooks(data_dir).unwrap();
        assert!(summaries.is_empty());
    }

    #[test]
    fn list_notebooks_returns_summaries() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let _a = create_notebook(data_dir, "Alpha").unwrap();
        let _b = create_notebook(data_dir, "Beta").unwrap();

        let summaries = list_notebooks(data_dir).unwrap();
        assert_eq!(summaries.len(), 2);

        let titles: Vec<&str> = summaries.iter().map(|s| s.title.as_str()).collect();
        assert!(titles.contains(&"Alpha"));
        assert!(titles.contains(&"Beta"));
    }

    #[test]
    fn list_notebooks_sorted_by_updated_at_descending() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let a = create_notebook(data_dir, "Older").unwrap();
        // Touch second notebook to ensure different updated_at.
        let _b = create_notebook(data_dir, "Newer").unwrap();

        // Update 'a' so it becomes the most recently updated.
        update_notebook(data_dir, &a.id, Some("Now Newest"), None).unwrap();

        let summaries = list_notebooks(data_dir).unwrap();
        assert_eq!(summaries[0].title, "Now Newest");
    }

    #[test]
    fn list_notebooks_skips_malformed_manifests() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        // Create one valid notebook.
        let _good = create_notebook(data_dir, "Good").unwrap();

        // Create a malformed notebook directory.
        let bad_dir = notebooks_dir(data_dir).join("not-a-uuid");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(bad_dir.join("ironpad.json"), "{ broken json").unwrap();

        let summaries = list_notebooks(data_dir).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].title, "Good");
    }

    #[test]
    fn list_notebooks_skips_non_directory_entries() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let _nb = create_notebook(data_dir, "Real").unwrap();

        // Create a stray file in the notebooks directory.
        std::fs::write(notebooks_dir(data_dir).join("stray.txt"), "oops").unwrap();

        let summaries = list_notebooks(data_dir).unwrap();
        assert_eq!(summaries.len(), 1);
    }

    #[test]
    fn list_notebooks_skips_dir_without_manifest() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let _nb = create_notebook(data_dir, "Real").unwrap();

        // Create a directory without ironpad.json.
        let orphan = notebooks_dir(data_dir).join("orphan-dir");
        std::fs::create_dir_all(&orphan).unwrap();

        let summaries = list_notebooks(data_dir).unwrap();
        assert_eq!(summaries.len(), 1);
    }

    #[test]
    fn round_trip_manifest_preserves_all_fields() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let created = create_notebook(data_dir, "Round Trip").unwrap();
        let cells = vec![CellManifest {
            id: "c1".to_string(),
            order: 0,
            label: "Label".to_string(),
            cell_type: CellType::default(),
        }];
        let updated =
            update_notebook(data_dir, &created.id, Some("Updated Title"), Some(cells)).unwrap();

        let reloaded = get_notebook(data_dir, &created.id).unwrap();

        assert_eq!(reloaded.id, updated.id);
        assert_eq!(reloaded.title, "Updated Title");
        assert_eq!(reloaded.compiler_version, "stable");
        assert_eq!(reloaded.cells.len(), 1);
        assert_eq!(reloaded.cells[0].id, "c1");
        assert_eq!(reloaded.cells[0].label, "Label");
    }

    #[test]
    fn notebook_summary_cell_count_matches() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let nb = create_notebook(data_dir, "With Cells").unwrap();
        let cells = vec![
            CellManifest {
                id: "c0".to_string(),
                order: 0,
                label: "A".to_string(),
                cell_type: CellType::default(),
            },
            CellManifest {
                id: "c1".to_string(),
                order: 1,
                label: "B".to_string(),
                cell_type: CellType::default(),
            },
            CellManifest {
                id: "c2".to_string(),
                order: 2,
                label: "C".to_string(),
                cell_type: CellType::default(),
            },
        ];
        update_notebook(data_dir, &nb.id, None, Some(cells)).unwrap();

        let summaries = list_notebooks(data_dir).unwrap();
        assert_eq!(summaries[0].cell_count, 3);
    }

    // ── Shared Cargo.toml ───────────────────────────────────────────────

    #[test]
    fn shared_cargo_toml_initially_none() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let nb = create_notebook(data_dir, "Test").unwrap();
        let result = get_shared_cargo_toml(data_dir, &nb.id).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn shared_cargo_toml_write_and_read() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let nb = create_notebook(data_dir, "Deps Test").unwrap();
        let content = "[dependencies]\nserde = \"1\"";

        update_shared_cargo_toml(data_dir, &nb.id, content).unwrap();

        let read = get_shared_cargo_toml(data_dir, &nb.id).unwrap();
        assert_eq!(read, Some(content.to_string()));
    }

    #[test]
    fn shared_cargo_toml_overwrite() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();

        let nb = create_notebook(data_dir, "Overwrite").unwrap();

        update_shared_cargo_toml(data_dir, &nb.id, "v1").unwrap();
        update_shared_cargo_toml(data_dir, &nb.id, "v2").unwrap();

        let read = get_shared_cargo_toml(data_dir, &nb.id).unwrap();
        assert_eq!(read, Some("v2".to_string()));
    }

    #[test]
    fn shared_cargo_toml_path_is_in_notebook_dir() {
        let data_dir = std::path::Path::new("/data");
        let id = Uuid::new_v4();
        let path = shared_cargo_toml_path(data_dir, &id);
        assert!(path.ends_with("Cargo.toml"));
        assert!(path.to_string_lossy().contains(&id.to_string()));
    }
}
