//! Sample notebook seeding for first-run experience.
//!
//! On startup, if the notebooks directory is empty, creates a "Welcome to
//! ironpad" notebook with two cells demonstrating the Fibonacci data-flow
//! example from the MegaPrd.

use std::path::Path;

use anyhow::{Context, Result};

use super::{cells, storage};

// ── Sample cell sources ──────────────────────────────────────────────────────

/// Cell 0: generate the first 20 Fibonacci numbers and output them as bincode.
const FIBONACCI_GENERATOR_SOURCE: &str = r#"    let fibs: Vec<u64> = {
        let mut v = vec![0, 1];
        for i in 2..20 {
            let next = v[i - 1] + v[i - 2];
            v.push(next);
        }
        v
    };

    CellOutput::new(&fibs).unwrap().with_display(format!("{fibs:?}")).into()
"#;

/// Cell 1: deserialize the Fibonacci numbers from the previous cell and sum them.
const FIBONACCI_CONSUMER_SOURCE: &str = r#"    let input = CellInput::new(unsafe { std::slice::from_raw_parts(input_ptr, input_len) });
    let fibs: Vec<u64> = input.deserialize().unwrap();
    let sum: u64 = fibs.iter().sum();

    CellOutput::text(format!("Sum of first {} Fibonacci numbers: {}", fibs.len(), sum)).into()
"#;

// ── Public API ───────────────────────────────────────────────────────────────

/// Seeds a sample "Welcome to ironpad" notebook if no notebooks exist yet.
///
/// Returns `Ok(true)` if the sample was created, `Ok(false)` if notebooks
/// already exist (nothing to do).
pub fn seed_sample_notebook(data_dir: &Path) -> Result<bool> {
    let existing =
        storage::list_notebooks(data_dir).context("failed to list notebooks during seed check")?;

    if !existing.is_empty() {
        tracing::debug!(
            count = existing.len(),
            "notebooks already exist, skipping seed"
        );
        return Ok(false);
    }

    tracing::info!("no notebooks found — seeding sample notebook");

    // Create the notebook.
    let notebook = storage::create_notebook(data_dir, "Welcome to ironpad")
        .context("failed to create sample notebook")?;

    // Add Cell 0: Fibonacci generator.
    let cell0 = cells::add_cell(
        data_dir,
        &notebook.id,
        "cell_0",
        "Fibonacci Generator",
        None,
    )
    .context("failed to add cell 0 to sample notebook")?;

    cells::update_cell_source(
        data_dir,
        &notebook.id,
        &cell0.id,
        FIBONACCI_GENERATOR_SOURCE,
    )
    .context("failed to write cell 0 source")?;

    // Add Cell 1: Fibonacci consumer (after cell 0).
    let cell1 = cells::add_cell(
        data_dir,
        &notebook.id,
        "cell_1",
        "Fibonacci Consumer",
        Some(&cell0.id),
    )
    .context("failed to add cell 1 to sample notebook")?;

    cells::update_cell_source(data_dir, &notebook.id, &cell1.id, FIBONACCI_CONSUMER_SOURCE)
        .context("failed to write cell 1 source")?;

    tracing::info!(
        id = %notebook.id,
        title = %notebook.title,
        "sample notebook seeded with 2 cells"
    );

    Ok(true)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_path_buf();
        (tmp, data_dir)
    }

    #[test]
    fn seeds_when_no_notebooks_exist() {
        let (_tmp, data_dir) = temp_data_dir();

        let created = seed_sample_notebook(&data_dir).unwrap();
        assert!(created);

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        assert_eq!(notebooks.len(), 1);
        assert_eq!(notebooks[0].title, "Welcome to ironpad");
        assert_eq!(notebooks[0].cell_count, 2);
    }

    #[test]
    fn skips_when_notebooks_already_exist() {
        let (_tmp, data_dir) = temp_data_dir();

        // Create a notebook first.
        storage::create_notebook(&data_dir, "Existing notebook").unwrap();

        let created = seed_sample_notebook(&data_dir).unwrap();
        assert!(!created);

        // Should still only have 1 notebook (the original).
        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        assert_eq!(notebooks.len(), 1);
        assert_eq!(notebooks[0].title, "Existing notebook");
    }

    #[test]
    fn seeded_notebook_has_correct_cell_sources() {
        let (_tmp, data_dir) = temp_data_dir();

        seed_sample_notebook(&data_dir).unwrap();

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        let nb = storage::get_notebook(&data_dir, &notebooks[0].id).unwrap();

        // Verify cell order and labels.
        assert_eq!(nb.cells.len(), 2);
        assert_eq!(nb.cells[0].id, "cell_0");
        assert_eq!(nb.cells[0].label, "Fibonacci Generator");
        assert_eq!(nb.cells[1].id, "cell_1");
        assert_eq!(nb.cells[1].label, "Fibonacci Consumer");

        // Verify cell 0 source.
        let src0 = cells::get_cell_source(&data_dir, &nb.id, "cell_0").unwrap();
        assert!(src0.contains("Vec<u64>"));
        assert!(src0.contains("CellOutput::new(&fibs)"));

        // Verify cell 1 source.
        let src1 = cells::get_cell_source(&data_dir, &nb.id, "cell_1").unwrap();
        assert!(src1.contains("input.deserialize()"));
        assert!(src1.contains("fibs.iter().sum()"));
    }

    #[test]
    fn seeded_notebook_has_valid_cargo_tomls() {
        let (_tmp, data_dir) = temp_data_dir();

        seed_sample_notebook(&data_dir).unwrap();

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        let nb = storage::get_notebook(&data_dir, &notebooks[0].id).unwrap();

        for cell in &nb.cells {
            let toml = cells::get_cell_cargo_toml(&data_dir, &nb.id, &cell.id).unwrap();
            assert!(toml.contains("[package]"));
            assert!(toml.contains("ironpad-cell"));
            assert!(toml.contains("cdylib"));
        }
    }

    #[test]
    fn idempotent_across_multiple_calls() {
        let (_tmp, data_dir) = temp_data_dir();

        // First call seeds.
        assert!(seed_sample_notebook(&data_dir).unwrap());

        // Second call is a no-op.
        assert!(!seed_sample_notebook(&data_dir).unwrap());

        // Still only one notebook.
        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        assert_eq!(notebooks.len(), 1);
    }
}
