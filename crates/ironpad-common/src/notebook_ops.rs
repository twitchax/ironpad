//! Structural cell operations shared by the browser model (the authority)
//! and the CLI daemon's event appliers.
//!
//! ONE definition per operation, on the [`NotebookMetaPatch::apply_to`]
//! pattern (PRD-0051): the daemon replays `Event`s against its cached
//! notebook, and any hand-mirrored copy of the model's insert/update/
//! reorder semantics silently diverges the agent's view from the browser's
//! the day one side changes. The metadata mirror already went through
//! exactly that lifecycle before it was unified.
//!
//! [`NotebookMetaPatch::apply_to`]: crate::protocol::NotebookMetaPatch::apply_to

use crate::types::IronpadCell;

/// Insert `cell` relative to `after_cell_id`, then renumber:
/// - `Some(id)` present → directly after that cell;
/// - `Some(id)` missing (deleted concurrently) → append;
/// - `None` → index 0 (the top "Add Cell" button).
pub fn insert_cell(cells: &mut Vec<IronpadCell>, cell: IronpadCell, after_cell_id: Option<&str>) {
    if let Some(after_id) = after_cell_id {
        if let Some(idx) = cells.iter().position(|c| c.id == after_id) {
            cells.insert(idx + 1, cell);
        } else {
            cells.push(cell);
        }
    } else {
        cells.insert(0, cell);
    }
    renumber(cells);
}

/// Remove a cell by id (a missing id is a no-op), then renumber.
pub fn delete_cell(cells: &mut Vec<IronpadCell>, cell_id: &str) {
    cells.retain(|c| c.id != cell_id);
    renumber(cells);
}

/// Reorder to match `cell_ids`, appending any cells the list omits
/// (shouldn't happen, but a concurrent add must not vanish), then renumber.
pub fn reorder_cells(cells: &mut Vec<IronpadCell>, cell_ids: &[String]) {
    let mut reordered = Vec::with_capacity(cell_ids.len());
    for id in cell_ids {
        if let Some(pos) = cells.iter().position(|c| &c.id == id) {
            reordered.push(cells.remove(pos));
        }
    }
    reordered.append(cells);
    *cells = reordered;
    renumber(cells);
}

/// Renumber every cell's `order` to its position.
pub fn renumber(cells: &mut [IronpadCell]) {
    for (i, cell) in cells.iter_mut().enumerate() {
        cell.order = u32::try_from(i).unwrap_or(u32::MAX);
    }
}

/// The changed-field bundle of `Mutation::CellUpdate` / `Event::CellUpdated`,
/// borrowed from either side's fields.
///
/// `cargo_toml` is the doubled option from the wire: `Some(None)` is an
/// explicit clear, `None` untouched.
pub struct CellPatch<'a> {
    pub source: Option<&'a String>,
    pub cargo_toml: Option<&'a Option<String>>,
    pub label: Option<&'a String>,
    pub shared: Option<bool>,
    pub collapsed: Option<bool>,
    pub output_collapsed: Option<bool>,
}

impl CellPatch<'_> {
    /// Apply the changed fields to `cell` and stamp `version`.
    pub fn apply_to(&self, cell: &mut IronpadCell, version: u64) {
        if let Some(src) = self.source {
            cell.source.clone_from(src);
        }
        if let Some(ct) = self.cargo_toml {
            cell.cargo_toml.clone_from(ct);
        }
        if let Some(lbl) = self.label {
            cell.label.clone_from(lbl);
        }
        if let Some(sh) = self.shared {
            cell.shared = sh;
        }
        if let Some(c) = self.collapsed {
            cell.collapsed = c;
        }
        if let Some(oc) = self.output_collapsed {
            cell.output_collapsed = oc;
        }
        cell.version = version;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(id: &str) -> IronpadCell {
        IronpadCell {
            id: id.to_string(),
            order: 0,
            label: id.to_string(),
            cell_type: crate::types::CellType::Code,
            source: "42".to_string(),
            cargo_toml: None,
            shared: false,
            collapsed: false,
            output_collapsed: false,
            version: 0,
            saved_output: None,
        }
    }

    fn ids(cells: &[IronpadCell]) -> Vec<&str> {
        cells.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn insert_covers_after_missing_and_top() {
        let mut cells = vec![cell("a"), cell("b")];
        insert_cell(&mut cells, cell("x"), Some("a"));
        assert_eq!(ids(&cells), ["a", "x", "b"]);
        // A vanished anchor appends rather than drops the cell.
        insert_cell(&mut cells, cell("y"), Some("gone"));
        assert_eq!(ids(&cells), ["a", "x", "b", "y"]);
        // None is the top Add button.
        insert_cell(&mut cells, cell("z"), None);
        assert_eq!(ids(&cells), ["z", "a", "x", "b", "y"]);
        let orders: Vec<u32> = cells.iter().map(|c| c.order).collect();
        assert_eq!(orders, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn reorder_appends_omitted_cells() {
        let mut cells = vec![cell("a"), cell("b"), cell("c")];
        reorder_cells(&mut cells, &["c".to_string(), "a".to_string()]);
        // "b" was omitted from the list; it must survive, appended.
        assert_eq!(ids(&cells), ["c", "a", "b"]);
        assert_eq!(cells[2].order, 2);
    }

    #[test]
    fn renumber_handles_empty_and_gapped_orders() {
        let mut cells: Vec<IronpadCell> = vec![];
        renumber(&mut cells);
        assert!(cells.is_empty());

        let mut cells = vec![cell("a"), cell("b"), cell("c")];
        cells[0].order = 10;
        cells[1].order = 5;
        cells[2].order = 99;
        renumber(&mut cells);
        let orders: Vec<u32> = cells.iter().map(|c| c.order).collect();
        assert_eq!(orders, [0, 1, 2]);
    }

    #[test]
    fn delete_renumbers_and_ignores_unknown() {
        let mut cells = vec![cell("a"), cell("b"), cell("c")];
        delete_cell(&mut cells, "b");
        assert_eq!(ids(&cells), ["a", "c"]);
        assert_eq!(cells[1].order, 1);
        delete_cell(&mut cells, "nope");
        assert_eq!(ids(&cells), ["a", "c"]);
    }

    #[test]
    fn patch_applies_only_changed_fields_and_stamps_version() {
        let mut c = cell("a");
        c.cargo_toml = Some("[dependencies]".to_string());
        let new_source = "43".to_string();
        CellPatch {
            source: Some(&new_source),
            // Some(None): the explicit clear must actually clear.
            cargo_toml: Some(&None),
            label: None,
            shared: Some(true),
            collapsed: None,
            output_collapsed: None,
        }
        .apply_to(&mut c, 7);
        assert_eq!(c.source, "43");
        assert_eq!(c.cargo_toml, None);
        assert_eq!(c.label, "a", "untouched fields stay");
        assert!(c.shared);
        assert_eq!(c.version, 7);
    }
}
