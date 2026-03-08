//! Sample notebook seeding for first-run experience.
//!
//! On startup, if the notebooks directory is empty, creates a "Welcome to
//! ironpad" notebook with four cells demonstrating the Fibonacci data-flow
//! example and a plotters chart.

use std::path::Path;

use anyhow::{Context, Result};

use super::{cells, storage};
use ironpad_common::CellType;

// ── Sample cell sources ──────────────────────────────────────────────────────

/// Cell 0: generate the first 20 Fibonacci numbers (bare expression → auto-output).
const FIBONACCI_GENERATOR_SOURCE: &str = r#"let fibs: Vec<u64> = {
    let mut v = vec![0, 1];
    for i in 2..20 {
        let next = v[i - 1] + v[i - 2];
        v.push(next);
    }
    v
};

fibs
"#;

/// Cell 1: filter to even Fibonacci numbers using typed `cell0` injection.
const FIBONACCI_FILTER_SOURCE: &str = r#"let evens: Vec<u64> = cell0
    .iter()
    .copied()
    .filter(|n| n % 2 == 0)
    .collect();

evens
"#;

/// Cell 2: summarize both previous cells — `cell0` (all fibs) and `cell1` (even fibs).
const FIBONACCI_SUMMARY_SOURCE: &str = r#"let total: u64 = cell0.iter().sum();
let even_total: u64 = cell1.iter().sum();

CellOutput::text(format!(
    "All {count} fibs sum to {total}. The {even_count} even fibs sum to {even_total}.",
    count = cell0.len(),
    even_count = cell1.len(),
))
"#;

/// Cell 3: bar chart of Fibonacci numbers using plotters (Svg rich output).
const FIBONACCI_CHART_SOURCE: &str = r#"use plotters::prelude::*;

let mut svg_buf = String::new();
{
    let root = SVGBackend::with_string(&mut svg_buf, (600, 400))
        .into_drawing_area();
    root.fill(&WHITE).unwrap();

    let max_val = *cell0.iter().max().unwrap_or(&1);

    let mut chart = ChartBuilder::on(&root)
        .caption("Fibonacci Sequence", ("sans-serif", 20))
        .margin(10)
        .x_label_area_size(30)
        .y_label_area_size(60)
        .build_cartesian_2d(0usize..cell0.len(), 0u64..max_val)
        .unwrap();

    chart.configure_mesh().draw().unwrap();

    chart
        .draw_series(cell0.iter().enumerate().map(|(x, &y)| {
            let color = if y % 2 == 0 { BLUE } else { RED };
            Rectangle::new([(x, 0u64), (x + 1, y)], color.filled())
        }))
        .unwrap()
        .label("Fibonacci values")
        .legend(|(x, y)| Rectangle::new([(x, y - 5), (x + 10, y + 5)], BLUE.filled()));

    chart.configure_series_labels().draw().unwrap();

    root.present().unwrap();
}

Svg(svg_buf)
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
        CellType::Code,
    )
    .context("failed to add cell 0 to sample notebook")?;

    cells::update_cell_source(
        data_dir,
        &notebook.id,
        &cell0.id,
        FIBONACCI_GENERATOR_SOURCE,
    )
    .context("failed to write cell 0 source")?;

    // Add Cell 1: Even Fibonacci filter (after cell 0).
    let cell1 = cells::add_cell(
        data_dir,
        &notebook.id,
        "cell_1",
        "Even Fibonacci Filter",
        Some(&cell0.id),
        CellType::Code,
    )
    .context("failed to add cell 1 to sample notebook")?;

    cells::update_cell_source(data_dir, &notebook.id, &cell1.id, FIBONACCI_FILTER_SOURCE)
        .context("failed to write cell 1 source")?;

    // Add Cell 2: Summary (after cell 1).
    let cell2 = cells::add_cell(
        data_dir,
        &notebook.id,
        "cell_2",
        "Summary",
        Some(&cell1.id),
        CellType::Code,
    )
    .context("failed to add cell 2 to sample notebook")?;

    cells::update_cell_source(data_dir, &notebook.id, &cell2.id, FIBONACCI_SUMMARY_SOURCE)
        .context("failed to write cell 2 source")?;

    // Add Cell 3: Fibonacci chart (after cell 2).
    let cell3 = cells::add_cell(
        data_dir,
        &notebook.id,
        "cell_3",
        "Fibonacci Chart",
        Some(&cell2.id),
        CellType::Code,
    )
    .context("failed to add cell 3 to sample notebook")?;

    cells::update_cell_source(data_dir, &notebook.id, &cell3.id, FIBONACCI_CHART_SOURCE)
        .context("failed to write cell 3 source")?;

    // Set shared Cargo.toml with plotters dependency.
    storage::update_shared_cargo_toml(
        data_dir,
        &notebook.id,
        r#"[dependencies]
plotters = "0.3"
"#,
    )
    .context("failed to write shared Cargo.toml")?;

    tracing::info!(
        id = %notebook.id,
        title = %notebook.title,
        "sample notebook seeded with 4 cells"
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
        assert_eq!(notebooks[0].cell_count, 4);
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
        assert_eq!(nb.cells.len(), 4);
        assert_eq!(nb.cells[0].id, "cell_0");
        assert_eq!(nb.cells[0].label, "Fibonacci Generator");
        assert_eq!(nb.cells[1].id, "cell_1");
        assert_eq!(nb.cells[1].label, "Even Fibonacci Filter");
        assert_eq!(nb.cells[2].id, "cell_2");
        assert_eq!(nb.cells[2].label, "Summary");
        assert_eq!(nb.cells[3].id, "cell_3");
        assert_eq!(nb.cells[3].label, "Fibonacci Chart");

        // Verify cell 0 source.
        let src0 = cells::get_cell_source(&data_dir, &nb.id, "cell_0").unwrap();
        assert!(src0.contains("Vec<u64>"));
        assert!(src0.contains("fibs"));
        assert!(!src0.contains("CellOutput::new"));

        // Verify cell 1 source.
        let src1 = cells::get_cell_source(&data_dir, &nb.id, "cell_1").unwrap();
        assert!(src1.contains("cell0"));
        assert!(src1.contains("filter"));
        assert!(src1.contains("evens"));

        // Verify cell 2 source.
        let src2 = cells::get_cell_source(&data_dir, &nb.id, "cell_2").unwrap();
        assert!(src2.contains("cell0"));
        assert!(src2.contains("cell1"));
        assert!(src2.contains("CellOutput::text"));

        // Verify cell 3 source.
        let src3 = cells::get_cell_source(&data_dir, &nb.id, "cell_3").unwrap();
        assert!(src3.contains("plotters"));
        assert!(src3.contains("SVGBackend"));
        assert!(src3.contains("Svg(svg_buf)"));

        // Verify shared Cargo.toml.
        let shared = storage::get_shared_cargo_toml(&data_dir, &nb.id).unwrap();
        assert!(shared.is_some());
        assert!(shared.unwrap().contains("plotters"));
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
