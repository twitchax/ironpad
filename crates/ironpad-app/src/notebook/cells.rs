//! Cell filesystem CRUD operations.
//!
//! Directory layout under `{notebook_dir}/cells/{cell_id}/`:
//! ```text
//! {cell_id}/
//!   source.rs             # user's Rust source code
//!   Cargo.toml            # cell-level dependency manifest
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ironpad_common::CellManifest;
use uuid::Uuid;

use super::storage;

// ── Path helpers ─────────────────────────────────────────────────────────────

/// Returns the top-level cells directory for a notebook.
pub fn cells_dir(data_dir: &Path, notebook_id: &Uuid) -> PathBuf {
    storage::notebook_dir(data_dir, notebook_id).join("cells")
}

/// Returns the directory for a specific cell.
pub fn cell_dir(data_dir: &Path, notebook_id: &Uuid, cell_id: &str) -> PathBuf {
    cells_dir(data_dir, notebook_id).join(cell_id)
}

/// Returns the path to a cell's source file.
pub fn source_path(data_dir: &Path, notebook_id: &Uuid, cell_id: &str) -> PathBuf {
    cell_dir(data_dir, notebook_id, cell_id).join("source.rs")
}

/// Returns the path to a cell's Cargo.toml file.
pub fn cargo_toml_path(data_dir: &Path, notebook_id: &Uuid, cell_id: &str) -> PathBuf {
    cell_dir(data_dir, notebook_id, cell_id).join("Cargo.toml")
}

// ── Default content ──────────────────────────────────────────────────────────

/// Default source code for a new cell.
const DEFAULT_SOURCE: &str = "    CellOutput::text(\"hello from ironpad\").into()\n";

/// Generates the default `Cargo.toml` for a cell (per MegaPrd §8.3).
fn default_cargo_toml(cell_id: &str) -> String {
    format!(
        r#"[package]
name = "{cell_id}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
ironpad-cell = "0.1"

# User adds their deps below this line:
"#
    )
}

// ── CRUD operations ──────────────────────────────────────────────────────────

/// Adds a new cell to a notebook.
///
/// Creates the cell directory with default `source.rs` and `Cargo.toml`,
/// and appends a `CellManifest` entry to the notebook manifest.
///
/// If `after_cell_id` is `Some`, the new cell is inserted after that cell;
/// otherwise it is appended at the end. Order fields are renumbered.
pub fn add_cell(
    data_dir: &Path,
    notebook_id: &Uuid,
    cell_id: &str,
    label: &str,
    after_cell_id: Option<&str>,
) -> Result<CellManifest> {
    let dir = cell_dir(data_dir, notebook_id, cell_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create cell directory: {}", dir.display()))?;

    // Write default files.
    let src = source_path(data_dir, notebook_id, cell_id);
    std::fs::write(&src, DEFAULT_SOURCE)
        .with_context(|| format!("Failed to write default source: {}", src.display()))?;

    let toml = cargo_toml_path(data_dir, notebook_id, cell_id);
    std::fs::write(&toml, default_cargo_toml(cell_id))
        .with_context(|| format!("Failed to write default Cargo.toml: {}", toml.display()))?;

    let cell = add_cell_to_manifest(data_dir, notebook_id, cell_id, label, after_cell_id)?;

    tracing::info!(notebook_id = %notebook_id, cell_id = %cell_id, "added cell");
    Ok(cell)
}

/// Reads a cell's source code from disk.
pub fn get_cell_source(data_dir: &Path, notebook_id: &Uuid, cell_id: &str) -> Result<String> {
    let path = source_path(data_dir, notebook_id, cell_id);
    std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read cell source: {}", path.display()))
}

/// Reads a cell's Cargo.toml from disk.
pub fn get_cell_cargo_toml(data_dir: &Path, notebook_id: &Uuid, cell_id: &str) -> Result<String> {
    let path = cargo_toml_path(data_dir, notebook_id, cell_id);
    std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read cell Cargo.toml: {}", path.display()))
}

/// Updates a cell's source code on disk.
pub fn update_cell_source(
    data_dir: &Path,
    notebook_id: &Uuid,
    cell_id: &str,
    source: &str,
) -> Result<()> {
    let path = source_path(data_dir, notebook_id, cell_id);

    anyhow::ensure!(
        path.parent().is_some_and(|p| p.exists()),
        "Cell directory does not exist: {}",
        cell_dir(data_dir, notebook_id, cell_id).display()
    );

    std::fs::write(&path, source)
        .with_context(|| format!("Failed to write cell source: {}", path.display()))?;

    // Bump notebook updated_at.
    storage::update_notebook(data_dir, notebook_id, None, None)?;

    tracing::info!(notebook_id = %notebook_id, cell_id = %cell_id, "updated cell source");
    Ok(())
}

/// Updates a cell's Cargo.toml on disk.
pub fn update_cell_cargo_toml(
    data_dir: &Path,
    notebook_id: &Uuid,
    cell_id: &str,
    cargo_toml: &str,
) -> Result<()> {
    let path = cargo_toml_path(data_dir, notebook_id, cell_id);

    anyhow::ensure!(
        path.parent().is_some_and(|p| p.exists()),
        "Cell directory does not exist: {}",
        cell_dir(data_dir, notebook_id, cell_id).display()
    );

    std::fs::write(&path, cargo_toml)
        .with_context(|| format!("Failed to write cell Cargo.toml: {}", path.display()))?;

    // Bump notebook updated_at.
    storage::update_notebook(data_dir, notebook_id, None, None)?;

    tracing::info!(notebook_id = %notebook_id, cell_id = %cell_id, "updated cell Cargo.toml");
    Ok(())
}

/// Deletes a cell's directory and removes it from the notebook manifest.
pub fn delete_cell(data_dir: &Path, notebook_id: &Uuid, cell_id: &str) -> Result<()> {
    // Remove from manifest first (so manifest is consistent even if dir removal fails).
    let mut manifest = storage::get_notebook(data_dir, notebook_id)?;
    manifest.cells.retain(|c| c.id != cell_id);
    renumber_cells(&mut manifest.cells);
    storage::update_notebook(data_dir, notebook_id, None, Some(manifest.cells))?;

    // Remove cell directory.
    let dir = cell_dir(data_dir, notebook_id, cell_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("Failed to delete cell directory: {}", dir.display()))?;
    }

    tracing::info!(notebook_id = %notebook_id, cell_id = %cell_id, "deleted cell");
    Ok(())
}

/// Reorders cells in the notebook manifest.
///
/// `cell_ids` must contain the IDs of all existing cells in the desired order.
/// Order fields are renumbered sequentially starting from 0.
pub fn reorder_cells(data_dir: &Path, notebook_id: &Uuid, cell_ids: &[String]) -> Result<()> {
    let manifest = storage::get_notebook(data_dir, notebook_id)?;

    // Build a lookup for existing cells.
    let lookup: std::collections::HashMap<&str, &CellManifest> =
        manifest.cells.iter().map(|c| (c.id.as_str(), c)).collect();

    // Validate that all provided IDs exist and no extras are included.
    anyhow::ensure!(
        cell_ids.len() == manifest.cells.len(),
        "cell_ids length ({}) does not match existing cell count ({})",
        cell_ids.len(),
        manifest.cells.len()
    );

    let mut reordered = Vec::with_capacity(cell_ids.len());
    for (order, id) in cell_ids.iter().enumerate() {
        let existing = lookup
            .get(id.as_str())
            .with_context(|| format!("Unknown cell ID in reorder list: {id}"))?;

        reordered.push(CellManifest {
            id: existing.id.clone(),
            order: order as u32,
            label: existing.label.clone(),
        });
    }

    storage::update_notebook(data_dir, notebook_id, None, Some(reordered))?;

    tracing::info!(notebook_id = %notebook_id, "reordered cells");
    Ok(())
}

/// Duplicates a cell, creating a new cell with copied source and Cargo.toml.
///
/// The new cell is inserted immediately after the source cell in the manifest.
/// A fresh UUID is assigned and the label gets a " (copy)" suffix.
pub fn duplicate_cell(
    data_dir: &Path,
    notebook_id: &Uuid,
    source_cell_id: &str,
    new_cell_id: &str,
) -> Result<CellManifest> {
    // Read the source cell's content.
    let src = get_cell_source(data_dir, notebook_id, source_cell_id)?;
    let toml = get_cell_cargo_toml(data_dir, notebook_id, source_cell_id)?;

    // Look up the source cell's label in the manifest.
    let manifest = storage::get_notebook(data_dir, notebook_id)?;
    let source_cell = manifest
        .cells
        .iter()
        .find(|c| c.id == source_cell_id)
        .with_context(|| format!("Cell not found: {source_cell_id}"))?;
    let new_label = format!("{} (copy)", source_cell.label);

    // Create the new cell directory with copied content.
    let dir = cell_dir(data_dir, notebook_id, new_cell_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create cell directory: {}", dir.display()))?;

    let src_path = source_path(data_dir, notebook_id, new_cell_id);
    std::fs::write(&src_path, &src)
        .with_context(|| format!("Failed to write duplicated source: {}", src_path.display()))?;

    // Rewrite the Cargo.toml [package] name to match the new cell ID.
    let new_toml = toml.replace(
        &format!("name = \"{source_cell_id}\""),
        &format!("name = \"{new_cell_id}\""),
    );

    let toml_path = cargo_toml_path(data_dir, notebook_id, new_cell_id);
    std::fs::write(&toml_path, &new_toml).with_context(|| {
        format!(
            "Failed to write duplicated Cargo.toml: {}",
            toml_path.display()
        )
    })?;

    // Insert the new cell into the manifest after the source cell.
    let cell = add_cell_to_manifest(
        data_dir,
        notebook_id,
        new_cell_id,
        &new_label,
        Some(source_cell_id),
    )?;

    tracing::info!(
        notebook_id = %notebook_id,
        source_cell_id = %source_cell_id,
        new_cell_id = %new_cell_id,
        "duplicated cell"
    );
    Ok(cell)
}

/// Renames a cell's label in the notebook manifest.
pub fn rename_cell(
    data_dir: &Path,
    notebook_id: &Uuid,
    cell_id: &str,
    new_label: &str,
) -> Result<()> {
    let mut manifest = storage::get_notebook(data_dir, notebook_id)?;

    let cell = manifest
        .cells
        .iter_mut()
        .find(|c| c.id == cell_id)
        .with_context(|| format!("Cell not found: {cell_id}"))?;

    cell.label = new_label.to_string();

    storage::update_notebook(data_dir, notebook_id, None, Some(manifest.cells))?;

    tracing::info!(notebook_id = %notebook_id, cell_id = %cell_id, label = %new_label, "renamed cell");
    Ok(())
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Inserts a new cell entry into the notebook manifest.
///
/// If `after_cell_id` is `Some`, inserts after that cell; otherwise appends.
/// Returns the resulting `CellManifest` with the correct order.
fn add_cell_to_manifest(
    data_dir: &Path,
    notebook_id: &Uuid,
    cell_id: &str,
    label: &str,
    after_cell_id: Option<&str>,
) -> Result<CellManifest> {
    let mut manifest = storage::get_notebook(data_dir, notebook_id)?;

    let insert_idx = match after_cell_id {
        Some(after_id) => manifest
            .cells
            .iter()
            .position(|c| c.id == after_id)
            .map(|pos| pos + 1)
            .unwrap_or(manifest.cells.len()),
        None => manifest.cells.len(),
    };

    let new_cell = CellManifest {
        id: cell_id.to_string(),
        order: 0, // renumbered below
        label: label.to_string(),
    };

    manifest.cells.insert(insert_idx, new_cell);
    renumber_cells(&mut manifest.cells);

    storage::update_notebook(data_dir, notebook_id, None, Some(manifest.cells))?;

    Ok(CellManifest {
        id: cell_id.to_string(),
        order: insert_idx as u32,
        label: label.to_string(),
    })
}

/// Renumbers cell order fields sequentially starting from 0.
fn renumber_cells(cells: &mut [CellManifest]) {
    for (i, cell) in cells.iter_mut().enumerate() {
        cell.order = i as u32;
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::storage::create_notebook;

    fn temp_data_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    // ── add_cell ─────────────────────────────────────────────────────────

    #[test]
    fn add_cell_creates_directory_and_files() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        let cell = add_cell(data_dir, &nb.id, "cell_0", "First Cell", None).unwrap();

        assert_eq!(cell.id, "cell_0");
        assert_eq!(cell.label, "First Cell");
        assert_eq!(cell.order, 0);

        // Verify files exist.
        assert!(source_path(data_dir, &nb.id, "cell_0").exists());
        assert!(cargo_toml_path(data_dir, &nb.id, "cell_0").exists());
    }

    #[test]
    fn add_cell_writes_default_source() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Cell", None).unwrap();

        let src = get_cell_source(data_dir, &nb.id, "cell_0").unwrap();
        assert!(src.contains("CellOutput"));
    }

    #[test]
    fn add_cell_writes_default_cargo_toml() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Cell", None).unwrap();

        let toml = get_cell_cargo_toml(data_dir, &nb.id, "cell_0").unwrap();
        assert!(toml.contains(r#"name = "cell_0""#));
        assert!(toml.contains(r#"crate-type = ["cdylib"]"#));
        assert!(toml.contains("ironpad-cell"));
    }

    #[test]
    fn add_cell_updates_manifest() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "First", None).unwrap();
        add_cell(data_dir, &nb.id, "cell_1", "Second", None).unwrap();

        let manifest = storage::get_notebook(data_dir, &nb.id).unwrap();
        assert_eq!(manifest.cells.len(), 2);
        assert_eq!(manifest.cells[0].id, "cell_0");
        assert_eq!(manifest.cells[0].order, 0);
        assert_eq!(manifest.cells[1].id, "cell_1");
        assert_eq!(manifest.cells[1].order, 1);
    }

    #[test]
    fn add_cell_after_inserts_at_correct_position() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "First", None).unwrap();
        add_cell(data_dir, &nb.id, "cell_2", "Third", None).unwrap();
        add_cell(data_dir, &nb.id, "cell_1", "Middle", Some("cell_0")).unwrap();

        let manifest = storage::get_notebook(data_dir, &nb.id).unwrap();
        assert_eq!(manifest.cells.len(), 3);
        assert_eq!(manifest.cells[0].id, "cell_0");
        assert_eq!(manifest.cells[1].id, "cell_1");
        assert_eq!(manifest.cells[2].id, "cell_2");
    }

    #[test]
    fn add_cell_after_unknown_id_appends() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "First", None).unwrap();
        add_cell(data_dir, &nb.id, "cell_1", "Second", Some("nonexistent")).unwrap();

        let manifest = storage::get_notebook(data_dir, &nb.id).unwrap();
        assert_eq!(manifest.cells.len(), 2);
        assert_eq!(manifest.cells[1].id, "cell_1");
    }

    // ── get_cell_source / get_cell_cargo_toml ────────────────────────────

    #[test]
    fn get_cell_source_reads_file() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Cell", None).unwrap();

        let source = get_cell_source(data_dir, &nb.id, "cell_0").unwrap();
        assert!(!source.is_empty());
    }

    #[test]
    fn get_cell_source_missing_returns_error() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        assert!(get_cell_source(data_dir, &nb.id, "nonexistent").is_err());
    }

    #[test]
    fn get_cell_cargo_toml_reads_file() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Cell", None).unwrap();

        let toml = get_cell_cargo_toml(data_dir, &nb.id, "cell_0").unwrap();
        assert!(toml.contains("[package]"));
    }

    // ── update_cell_source ───────────────────────────────────────────────

    #[test]
    fn update_cell_source_overwrites_file() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Cell", None).unwrap();

        update_cell_source(data_dir, &nb.id, "cell_0", "let x = 42;\n").unwrap();

        let src = get_cell_source(data_dir, &nb.id, "cell_0").unwrap();
        assert_eq!(src, "let x = 42;\n");
    }

    #[test]
    fn update_cell_source_bumps_updated_at() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Cell", None).unwrap();
        let before = storage::get_notebook(data_dir, &nb.id).unwrap().updated_at;

        update_cell_source(data_dir, &nb.id, "cell_0", "new code\n").unwrap();
        let after = storage::get_notebook(data_dir, &nb.id).unwrap().updated_at;

        assert!(after >= before);
    }

    #[test]
    fn update_cell_source_missing_cell_returns_error() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        assert!(update_cell_source(data_dir, &nb.id, "nonexistent", "code").is_err());
    }

    // ── update_cell_cargo_toml ───────────────────────────────────────────

    #[test]
    fn update_cell_cargo_toml_overwrites_file() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Cell", None).unwrap();

        let new_toml = "[package]\nname = \"custom\"\n";
        update_cell_cargo_toml(data_dir, &nb.id, "cell_0", new_toml).unwrap();

        let toml = get_cell_cargo_toml(data_dir, &nb.id, "cell_0").unwrap();
        assert_eq!(toml, new_toml);
    }

    #[test]
    fn update_cell_cargo_toml_missing_cell_returns_error() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        assert!(update_cell_cargo_toml(data_dir, &nb.id, "nonexistent", "toml").is_err());
    }

    // ── delete_cell ──────────────────────────────────────────────────────

    #[test]
    fn delete_cell_removes_directory() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Doomed", None).unwrap();
        assert!(cell_dir(data_dir, &nb.id, "cell_0").exists());

        delete_cell(data_dir, &nb.id, "cell_0").unwrap();
        assert!(!cell_dir(data_dir, &nb.id, "cell_0").exists());
    }

    #[test]
    fn delete_cell_updates_manifest() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "First", None).unwrap();
        add_cell(data_dir, &nb.id, "cell_1", "Second", None).unwrap();

        delete_cell(data_dir, &nb.id, "cell_0").unwrap();

        let manifest = storage::get_notebook(data_dir, &nb.id).unwrap();
        assert_eq!(manifest.cells.len(), 1);
        assert_eq!(manifest.cells[0].id, "cell_1");
        assert_eq!(manifest.cells[0].order, 0); // renumbered
    }

    #[test]
    fn delete_cell_missing_directory_still_updates_manifest() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Ghost", None).unwrap();

        // Manually remove the directory.
        std::fs::remove_dir_all(cell_dir(data_dir, &nb.id, "cell_0")).unwrap();

        // Should still succeed (manifest cleanup).
        delete_cell(data_dir, &nb.id, "cell_0").unwrap();

        let manifest = storage::get_notebook(data_dir, &nb.id).unwrap();
        assert!(manifest.cells.is_empty());
    }

    // ── reorder_cells ────────────────────────────────────────────────────

    #[test]
    fn reorder_cells_changes_order() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_a", "A", None).unwrap();
        add_cell(data_dir, &nb.id, "cell_b", "B", None).unwrap();
        add_cell(data_dir, &nb.id, "cell_c", "C", None).unwrap();

        reorder_cells(
            data_dir,
            &nb.id,
            &["cell_c".into(), "cell_a".into(), "cell_b".into()],
        )
        .unwrap();

        let manifest = storage::get_notebook(data_dir, &nb.id).unwrap();
        assert_eq!(manifest.cells[0].id, "cell_c");
        assert_eq!(manifest.cells[0].order, 0);
        assert_eq!(manifest.cells[1].id, "cell_a");
        assert_eq!(manifest.cells[1].order, 1);
        assert_eq!(manifest.cells[2].id, "cell_b");
        assert_eq!(manifest.cells[2].order, 2);
    }

    #[test]
    fn reorder_cells_preserves_labels() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_a", "Alpha", None).unwrap();
        add_cell(data_dir, &nb.id, "cell_b", "Beta", None).unwrap();

        reorder_cells(data_dir, &nb.id, &["cell_b".into(), "cell_a".into()]).unwrap();

        let manifest = storage::get_notebook(data_dir, &nb.id).unwrap();
        assert_eq!(manifest.cells[0].label, "Beta");
        assert_eq!(manifest.cells[1].label, "Alpha");
    }

    #[test]
    fn reorder_cells_wrong_count_returns_error() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_a", "A", None).unwrap();
        add_cell(data_dir, &nb.id, "cell_b", "B", None).unwrap();

        // Too few IDs.
        assert!(reorder_cells(data_dir, &nb.id, &["cell_a".into()]).is_err());
    }

    #[test]
    fn reorder_cells_unknown_id_returns_error() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_a", "A", None).unwrap();

        assert!(reorder_cells(data_dir, &nb.id, &["unknown".into()]).is_err());
    }

    // ── rename_cell ──────────────────────────────────────────────────────

    #[test]
    fn rename_cell_updates_label() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        add_cell(data_dir, &nb.id, "cell_0", "Old Label", None).unwrap();

        rename_cell(data_dir, &nb.id, "cell_0", "New Label").unwrap();

        let manifest = storage::get_notebook(data_dir, &nb.id).unwrap();
        assert_eq!(manifest.cells[0].label, "New Label");
    }

    #[test]
    fn rename_cell_missing_returns_error() {
        let tmp = temp_data_dir();
        let data_dir = tmp.path();
        let nb = create_notebook(data_dir, "Test").unwrap();

        assert!(rename_cell(data_dir, &nb.id, "nonexistent", "Label").is_err());
    }

    // ── default content ──────────────────────────────────────────────────

    #[test]
    fn default_cargo_toml_contains_required_fields() {
        let toml = default_cargo_toml("my_cell");
        assert!(toml.contains(r#"name = "my_cell""#));
        assert!(toml.contains(r#"crate-type = ["cdylib"]"#));
        assert!(toml.contains("ironpad-cell"));
        assert!(toml.contains("edition = \"2021\""));
    }

    #[test]
    fn default_cargo_toml_substitutes_cell_id() {
        let toml = default_cargo_toml("cell_42");
        assert!(toml.contains(r#"name = "cell_42""#));
    }

    // ── renumber_cells helper ────────────────────────────────────────────

    #[test]
    fn renumber_cells_assigns_sequential_orders() {
        let mut cells = vec![
            CellManifest {
                id: "a".into(),
                order: 99,
                label: "A".into(),
            },
            CellManifest {
                id: "b".into(),
                order: 50,
                label: "B".into(),
            },
            CellManifest {
                id: "c".into(),
                order: 7,
                label: "C".into(),
            },
        ];

        renumber_cells(&mut cells);

        assert_eq!(cells[0].order, 0);
        assert_eq!(cells[1].order, 1);
        assert_eq!(cells[2].order, 2);
    }
}
