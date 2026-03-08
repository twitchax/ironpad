//! Sample notebook seeding for first-run experience.
//!
//! On startup, if the notebooks directory is empty, creates a gallery of seed
//! notebooks demonstrating ironpad features: Fibonacci data-flow, async HTTP,
//! and an interactive tutorial.

use std::path::Path;

use anyhow::{Context, Result};

use super::{cells, storage};
use ironpad_common::CellType;

// ── Seed data structures ─────────────────────────────────────────────────────

/// A seed notebook definition.
struct SeedNotebook {
    title: String,
    cells: Vec<SeedCell>,
    shared_cargo_toml: Option<String>,
}

/// A single cell within a seed notebook.
struct SeedCell {
    label: String,
    cell_type: CellType,
    source: String,
}

/// Returns all seed notebooks to create on first run.
fn all_seeds() -> Vec<SeedNotebook> {
    vec![
        welcome_notebook(),
        async_http_notebook(),
        tutorial_notebook(),
    ]
}

// ── Notebook 1: Welcome to ironpad ──────────────────────────────────────────

fn welcome_notebook() -> SeedNotebook {
    SeedNotebook {
        title: "Welcome to ironpad".to_string(),
        shared_cargo_toml: Some(
            r#"[dependencies]
plotters = "0.3"
"#
            .to_string(),
        ),
        cells: vec![
            SeedCell {
                label: "Fibonacci Generator".to_string(),
                cell_type: CellType::Code,
                source: r#"let fibs: Vec<u64> = {
    let mut v = vec![0, 1];
    for i in 2..20 {
        let next = v[i - 1] + v[i - 2];
        v.push(next);
    }
    v
};

fibs
"#
                .to_string(),
            },
            SeedCell {
                label: "Even Fibonacci Filter".to_string(),
                cell_type: CellType::Code,
                source: r#"let evens: Vec<u64> = cell0
    .iter()
    .copied()
    .filter(|n| n % 2 == 0)
    .collect();

evens
"#
                .to_string(),
            },
            SeedCell {
                label: "Summary".to_string(),
                cell_type: CellType::Code,
                source: r#"let total: u64 = cell0.iter().sum();
let even_total: u64 = cell1.iter().sum();

CellOutput::text(format!(
    "All {count} fibs sum to {total}. The {even_count} even fibs sum to {even_total}.",
    count = cell0.len(),
    even_count = cell1.len(),
))
"#
                .to_string(),
            },
            SeedCell {
                label: "Fibonacci Chart".to_string(),
                cell_type: CellType::Code,
                source: r#"use plotters::prelude::*;

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
"#
                .to_string(),
            },
        ],
    }
}

// ── Notebook 2: Async & HTTP ────────────────────────────────────────────────

fn async_http_notebook() -> SeedNotebook {
    SeedNotebook {
        title: "Async & HTTP".to_string(),
        shared_cargo_toml: Some(
            r#"[dependencies]
serde_json = "1"
"#
            .to_string(),
        ),
        cells: vec![
            SeedCell {
                label: "Intro".to_string(),
                cell_type: CellType::Markdown,
                source: r#"# Fetching Data from the Web

This notebook demonstrates making HTTP requests from ironpad cells using async Rust.
Cells that use `.await` are automatically compiled as async functions."#
                    .to_string(),
            },
            SeedCell {
                label: "HTTP GET".to_string(),
                cell_type: CellType::Code,
                source: r#"// Fetch a fun fact from a public API
let response = ironpad_cell::http::get("https://httpbin.org/get").await.unwrap();

// The response is a JSON string
format!("Response length: {} bytes\n\nFirst 500 chars:\n{}", response.len(), &response[..500.min(response.len())])
"#
                .to_string(),
            },
            SeedCell {
                label: "Parsing JSON".to_string(),
                cell_type: CellType::Markdown,
                source: r#"## Parsing JSON

You can deserialize JSON responses into Rust types using `serde_json`."#
                    .to_string(),
            },
            SeedCell {
                label: "JSON Parse".to_string(),
                cell_type: CellType::Code,
                source: r#"use serde_json::Value;

// Parse raw JSON
let data: Value = serde_json::from_str(&cell0).unwrap();

// Extract fields
let origin = data["origin"].as_str().unwrap_or("unknown");
let url = data["url"].as_str().unwrap_or("unknown");

Html(format!(
    "<table style='border-collapse:collapse; width:100%;'>
        <tr><th style='border:1px solid #0f3460; padding:8px; text-align:left; background:#1a1a2e;'>Field</th>
            <th style='border:1px solid #0f3460; padding:8px; text-align:left; background:#1a1a2e;'>Value</th></tr>
        <tr><td style='border:1px solid #0f3460; padding:8px;'>Origin IP</td>
            <td style='border:1px solid #0f3460; padding:8px;'>{origin}</td></tr>
        <tr><td style='border:1px solid #0f3460; padding:8px;'>URL</td>
            <td style='border:1px solid #0f3460; padding:8px;'>{url}</td></tr>
    </table>"
))
"#
                .to_string(),
            },
        ],
    }
}

// ── Notebook 3: Interactive Tutorial ────────────────────────────────────────

fn tutorial_notebook() -> SeedNotebook {
    SeedNotebook {
        title: "Interactive Tutorial".to_string(),
        shared_cargo_toml: None,
        cells: vec![
            SeedCell {
                label: "Welcome".to_string(),
                cell_type: CellType::Markdown,
                source: r#"# Welcome to ironpad! 🦀

**ironpad** is an interactive Rust notebook that compiles each cell to WebAssembly and runs it in your browser.

## Getting Started
- Each **code cell** contains Rust code that gets compiled and executed
- The last expression in a cell becomes its output
- Outputs are automatically passed to later cells as typed variables"#
                    .to_string(),
            },
            SeedCell {
                label: "Simple Expression".to_string(),
                cell_type: CellType::Code,
                source: r#"// Just type an expression — it becomes the cell's output!
// This cell outputs a Vec<i32>.
vec![1, 1, 2, 3, 5, 8, 13, 21]
"#
                .to_string(),
            },
            SeedCell {
                label: "Cell Variables".to_string(),
                cell_type: CellType::Markdown,
                source: r#"## Cell Variables

Each code cell's output is available in later cells as a typed variable:
- `cell0`, `cell1`, ... — by cell index (only counting code cells)
- `last` — alias for the most recent code cell's output

The variables are strongly typed — the compiler knows their Rust types!"#
                    .to_string(),
            },
            SeedCell {
                label: "Using Variables".to_string(),
                cell_type: CellType::Code,
                source: r#"// `cell0` is automatically available as Vec<i32>
let sum: i32 = cell0.iter().sum();
let count = cell0.len();

format!("The {} Fibonacci numbers sum to {}", count, sum)
"#
                .to_string(),
            },
            SeedCell {
                label: "Rich Output".to_string(),
                cell_type: CellType::Markdown,
                source: r#"## Rich Output

Cells can output more than just text. Use `Html(...)` for HTML or `Svg(...)` for SVG graphics.
You can even combine multiple outputs using tuples!"#
                    .to_string(),
            },
            SeedCell {
                label: "HTML Output".to_string(),
                cell_type: CellType::Code,
                source: r#"Html(format!(
    "<div style='padding:1rem; background:linear-gradient(135deg, #1a1a2e, #16213e); border-radius:8px; text-align:center;'>
        <h2 style='color:#e94560; margin:0;'>Hello from ironpad!</h2>
        <p style='color:#eaeaea;'>This is <strong>rich HTML</strong> output 🎨</p>
        <p style='color:#8888aa; font-size:0.9em;'>Sum from previous cell: <code style='color:#e94560;'>{}</code></p>
    </div>",
    last
))
"#
                .to_string(),
            },
        ],
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Seeds sample notebooks if no notebooks exist yet.
///
/// Returns `Ok(true)` if the samples were created, `Ok(false)` if notebooks
/// already exist (nothing to do).
pub fn seed_sample_notebooks(data_dir: &Path) -> Result<bool> {
    let existing =
        storage::list_notebooks(data_dir).context("failed to list notebooks during seed check")?;

    if !existing.is_empty() {
        tracing::debug!(
            count = existing.len(),
            "notebooks already exist, skipping seed"
        );
        return Ok(false);
    }

    tracing::info!("no notebooks found — seeding sample notebooks");

    for seed in all_seeds() {
        create_seed_notebook(data_dir, &seed)?;
    }

    Ok(true)
}

/// Creates a single seed notebook from a `SeedNotebook` definition.
fn create_seed_notebook(data_dir: &Path, seed: &SeedNotebook) -> Result<()> {
    let notebook = storage::create_notebook(data_dir, &seed.title)
        .with_context(|| format!("failed to create seed notebook '{}'", seed.title))?;

    let mut prev_cell_id: Option<String> = None;

    for (i, cell) in seed.cells.iter().enumerate() {
        let cell_id = format!("cell_{i}");

        let manifest = cells::add_cell(
            data_dir,
            &notebook.id,
            &cell_id,
            &cell.label,
            prev_cell_id.as_deref(),
            cell.cell_type.clone(),
        )
        .with_context(|| {
            format!(
                "failed to add cell {i} '{}' to seed notebook '{}'",
                cell.label, seed.title
            )
        })?;

        cells::update_cell_source(data_dir, &notebook.id, &manifest.id, &cell.source)
            .with_context(|| {
                format!(
                    "failed to write source for cell {i} in seed notebook '{}'",
                    seed.title
                )
            })?;

        prev_cell_id = Some(manifest.id);
    }

    if let Some(cargo_toml) = &seed.shared_cargo_toml {
        storage::update_shared_cargo_toml(data_dir, &notebook.id, cargo_toml).with_context(
            || {
                format!(
                    "failed to write shared Cargo.toml for seed notebook '{}'",
                    seed.title
                )
            },
        )?;
    }

    tracing::info!(
        id = %notebook.id,
        title = %seed.title,
        cells = seed.cells.len(),
        "seed notebook created"
    );

    Ok(())
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
    fn seeds_all_notebooks_when_none_exist() {
        let (_tmp, data_dir) = temp_data_dir();

        let created = seed_sample_notebooks(&data_dir).unwrap();
        assert!(created);

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        assert_eq!(notebooks.len(), 3);

        let titles: Vec<&str> = notebooks.iter().map(|n| n.title.as_str()).collect();
        assert!(titles.contains(&"Welcome to ironpad"));
        assert!(titles.contains(&"Async & HTTP"));
        assert!(titles.contains(&"Interactive Tutorial"));
    }

    #[test]
    fn skips_when_notebooks_already_exist() {
        let (_tmp, data_dir) = temp_data_dir();

        storage::create_notebook(&data_dir, "Existing notebook").unwrap();

        let created = seed_sample_notebooks(&data_dir).unwrap();
        assert!(!created);

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        assert_eq!(notebooks.len(), 1);
        assert_eq!(notebooks[0].title, "Existing notebook");
    }

    #[test]
    fn welcome_notebook_has_correct_cells() {
        let (_tmp, data_dir) = temp_data_dir();

        seed_sample_notebooks(&data_dir).unwrap();

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        let nb = notebooks
            .iter()
            .find(|n| n.title == "Welcome to ironpad")
            .unwrap();
        let nb = storage::get_notebook(&data_dir, &nb.id).unwrap();

        assert_eq!(nb.cells.len(), 4);
        assert_eq!(nb.cells[0].label, "Fibonacci Generator");
        assert_eq!(nb.cells[1].label, "Even Fibonacci Filter");
        assert_eq!(nb.cells[2].label, "Summary");
        assert_eq!(nb.cells[3].label, "Fibonacci Chart");

        // All cells should be Code type.
        for cell in &nb.cells {
            assert_eq!(cell.cell_type, CellType::Code);
        }

        // Verify cell sources.
        let src0 = cells::get_cell_source(&data_dir, &nb.id, "cell_0").unwrap();
        assert!(src0.contains("Vec<u64>"));
        assert!(src0.contains("fibs"));

        let src3 = cells::get_cell_source(&data_dir, &nb.id, "cell_3").unwrap();
        assert!(src3.contains("plotters"));
        assert!(src3.contains("Svg(svg_buf)"));

        // Verify shared Cargo.toml.
        let shared = storage::get_shared_cargo_toml(&data_dir, &nb.id).unwrap();
        assert!(shared.is_some());
        assert!(shared.unwrap().contains("plotters"));
    }

    #[test]
    fn async_http_notebook_has_correct_cells() {
        let (_tmp, data_dir) = temp_data_dir();

        seed_sample_notebooks(&data_dir).unwrap();

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        let nb = notebooks
            .iter()
            .find(|n| n.title == "Async & HTTP")
            .unwrap();
        let nb = storage::get_notebook(&data_dir, &nb.id).unwrap();

        assert_eq!(nb.cells.len(), 4);

        // Verify cell types: Markdown, Code, Markdown, Code.
        assert_eq!(nb.cells[0].cell_type, CellType::Markdown);
        assert_eq!(nb.cells[1].cell_type, CellType::Code);
        assert_eq!(nb.cells[2].cell_type, CellType::Markdown);
        assert_eq!(nb.cells[3].cell_type, CellType::Code);

        // HTTP cell uses .await for async detection.
        let src1 = cells::get_cell_source(&data_dir, &nb.id, "cell_1").unwrap();
        assert!(src1.contains(".await"));
        assert!(src1.contains("ironpad_cell::http::get"));

        // JSON parse cell references cell0 (first code cell output).
        let src3 = cells::get_cell_source(&data_dir, &nb.id, "cell_3").unwrap();
        assert!(src3.contains("serde_json"));
        assert!(src3.contains("cell0"));

        // Shared deps include serde_json.
        let shared = storage::get_shared_cargo_toml(&data_dir, &nb.id).unwrap();
        assert!(shared.is_some());
        assert!(shared.unwrap().contains("serde_json"));
    }

    #[test]
    fn tutorial_notebook_has_correct_cells() {
        let (_tmp, data_dir) = temp_data_dir();

        seed_sample_notebooks(&data_dir).unwrap();

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        let nb = notebooks
            .iter()
            .find(|n| n.title == "Interactive Tutorial")
            .unwrap();
        let nb = storage::get_notebook(&data_dir, &nb.id).unwrap();

        assert_eq!(nb.cells.len(), 6);

        // Verify cell types: Md, Code, Md, Code, Md, Code.
        assert_eq!(nb.cells[0].cell_type, CellType::Markdown);
        assert_eq!(nb.cells[1].cell_type, CellType::Code);
        assert_eq!(nb.cells[2].cell_type, CellType::Markdown);
        assert_eq!(nb.cells[3].cell_type, CellType::Code);
        assert_eq!(nb.cells[4].cell_type, CellType::Markdown);
        assert_eq!(nb.cells[5].cell_type, CellType::Code);

        // Tutorial has no shared deps.
        let shared = storage::get_shared_cargo_toml(&data_dir, &nb.id).unwrap();
        assert!(shared.is_none());

        // Code cell uses cell0 variable.
        let src3 = cells::get_cell_source(&data_dir, &nb.id, "cell_3").unwrap();
        assert!(src3.contains("cell0"));
        assert!(src3.contains("iter().sum()"));

        // HTML output cell.
        let src5 = cells::get_cell_source(&data_dir, &nb.id, "cell_5").unwrap();
        assert!(src5.contains("Html("));
        assert!(src5.contains("last"));
    }

    #[test]
    fn seeded_notebooks_have_valid_cargo_tomls() {
        let (_tmp, data_dir) = temp_data_dir();

        seed_sample_notebooks(&data_dir).unwrap();

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        for summary in &notebooks {
            let nb = storage::get_notebook(&data_dir, &summary.id).unwrap();
            for cell in &nb.cells {
                let toml = cells::get_cell_cargo_toml(&data_dir, &nb.id, &cell.id).unwrap();
                assert!(toml.contains("[package]"));
                assert!(toml.contains("ironpad-cell"));
                assert!(toml.contains("cdylib"));
            }
        }
    }

    #[test]
    fn idempotent_across_multiple_calls() {
        let (_tmp, data_dir) = temp_data_dir();

        assert!(seed_sample_notebooks(&data_dir).unwrap());
        assert!(!seed_sample_notebooks(&data_dir).unwrap());

        let notebooks = storage::list_notebooks(&data_dir).unwrap();
        assert_eq!(notebooks.len(), 3);
    }
}
