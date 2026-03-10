//! Ergonomic plotting API wrapping plotters' `SVGBackend`.

use plotters::prelude::*;

use crate::{CellOutput, DisplayPanel, IntoPanels, TypeTag};

// ── Dark-theme palette ───────────────────────────────────────────────────────

const COLOR_TEXT: RGBColor = RGBColor(0xEA, 0xEA, 0xEA);
const COLOR_ACCENT: RGBColor = RGBColor(0xE9, 0x45, 0x60);
const COLOR_TRANSPARENT: RGBColor = RGBColor(0, 0, 0);

// ── Plot builder ─────────────────────────────────────────────────────────────

/// Chart variant stored inside `Plot`.
#[derive(serde::Serialize, serde::Deserialize)]
enum ChartKind {
    Line(Vec<(f64, f64)>),
    Bar(Vec<(String, f64)>),
    Scatter(Vec<(f64, f64)>),
}

/// Builder for creating charts rendered to SVG.
///
/// # Examples
///
/// ```ignore
/// let plot = Plot::line(&[(0.0, 1.0), (1.0, 4.0), (2.0, 9.0)])
///     .title("Quadratic")
///     .x_label("x")
///     .y_label("y");
/// ```
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Plot {
    kind: ChartKind,
    title: Option<String>,
    x_label: Option<String>,
    y_label: Option<String>,
    width: u32,
    height: u32,
}

impl Plot {
    /// Create a line chart from `(x, y)` data points.
    #[must_use]
    pub fn line(data: &[(f64, f64)]) -> Self {
        Self {
            kind: ChartKind::Line(data.to_vec()),
            title: None,
            x_label: None,
            y_label: None,
            width: 800,
            height: 400,
        }
    }

    /// Create a bar chart from `(label, value)` data points.
    #[must_use]
    pub fn bar(data: &[(&str, f64)]) -> Self {
        Self {
            kind: ChartKind::Bar(data.iter().map(|(l, v)| ((*l).to_owned(), *v)).collect()),
            title: None,
            x_label: None,
            y_label: None,
            width: 800,
            height: 400,
        }
    }

    /// Create a scatter plot from `(x, y)` data points.
    #[must_use]
    pub fn scatter(data: &[(f64, f64)]) -> Self {
        Self {
            kind: ChartKind::Scatter(data.to_vec()),
            title: None,
            x_label: None,
            y_label: None,
            width: 800,
            height: 400,
        }
    }

    /// Set the chart title.
    #[must_use]
    pub fn title(mut self, title: &str) -> Self {
        self.title = Some(title.to_owned());
        self
    }

    /// Set the x-axis label.
    #[must_use]
    pub fn x_label(mut self, label: &str) -> Self {
        self.x_label = Some(label.to_owned());
        self
    }

    /// Set the y-axis label.
    #[must_use]
    pub fn y_label(mut self, label: &str) -> Self {
        self.y_label = Some(label.to_owned());
        self
    }

    /// Set the chart dimensions in pixels (default: 800×400).
    #[must_use]
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Render the chart to an SVG string.
    fn render_svg(&self) -> String {
        let mut buf = String::new();

        {
            let root =
                SVGBackend::with_string(&mut buf, (self.width, self.height)).into_drawing_area();

            // Transparent background (writes black fill, but we strip it below).
            root.fill(&COLOR_TRANSPARENT).unwrap();

            match &self.kind {
                ChartKind::Line(data) => self.render_line(&root, data),
                ChartKind::Bar(data) => self.render_bar(&root, data),
                ChartKind::Scatter(data) => self.render_scatter(&root, data),
            }

            root.present().unwrap();
        }

        // Make the background truly transparent by replacing the initial rect fill.
        buf.replace("fill=\"#000000\"", "fill=\"transparent\"")
    }

    // ── Per-kind renderers ───────────────────────────────────────────────

    fn render_line(
        &self,
        root: &DrawingArea<SVGBackend<'_>, plotters::coord::Shift>,
        data: &[(f64, f64)],
    ) {
        let (x_range, y_range) = xy_ranges(data);

        let mut chart = self.build_chart_context(root, x_range, y_range);

        chart
            .draw_series(LineSeries::new(
                data.iter().copied(),
                COLOR_ACCENT.stroke_width(2),
            ))
            .unwrap();
    }

    fn render_bar(
        &self,
        root: &DrawingArea<SVGBackend<'_>, plotters::coord::Shift>,
        data: &[(String, f64)],
    ) {
        if data.is_empty() {
            return;
        }

        let max_val = data
            .iter()
            .map(|(_, v)| *v)
            .fold(f64::NEG_INFINITY, f64::max);
        let y_top = if max_val <= 0.0 { 1.0 } else { max_val * 1.1 };
        let n = data.len() as f64;

        let mut builder = ChartBuilder::on(root);
        builder.margin(10);

        if let Some(t) = &self.title {
            builder.caption(
                t.as_str(),
                ("sans-serif", 18).into_font().color(&COLOR_TEXT),
            );
        }

        builder.set_label_area_size(LabelAreaPosition::Bottom, 40);
        builder.set_label_area_size(LabelAreaPosition::Left, 60);

        let mut chart = builder.build_cartesian_2d(0.0..n, 0.0..y_top).unwrap();

        chart
            .configure_mesh()
            .disable_x_mesh()
            .x_label_formatter(&|x| {
                let idx = *x as usize;
                data.get(idx).map_or_else(String::new, |(l, _)| l.clone())
            })
            .x_labels(data.len())
            .label_style(("sans-serif", 12).into_font().color(&COLOR_TEXT))
            .axis_style(COLOR_TEXT)
            .draw()
            .unwrap();

        chart
            .draw_series(data.iter().enumerate().map(|(i, (_, val))| {
                let x0 = i as f64 + 0.1;
                let x1 = (i + 1) as f64 - 0.1;
                let mut bar = Rectangle::new([(x0, 0.0), (x1, *val)], COLOR_ACCENT.filled());
                bar.set_margin(0, 0, 2, 2);
                bar
            }))
            .unwrap();
    }

    fn render_scatter(
        &self,
        root: &DrawingArea<SVGBackend<'_>, plotters::coord::Shift>,
        data: &[(f64, f64)],
    ) {
        let (x_range, y_range) = xy_ranges(data);

        let mut chart = self.build_chart_context(root, x_range, y_range);

        chart
            .draw_series(
                data.iter()
                    .map(|(x, y)| Circle::new((*x, *y), 4, COLOR_ACCENT.filled())),
            )
            .unwrap();
    }

    // ── Shared chart builder helper ──────────────────────────────────────

    fn build_chart_context<'a, 'b>(
        &self,
        root: &'a DrawingArea<SVGBackend<'b>, plotters::coord::Shift>,
        x_range: std::ops::Range<f64>,
        y_range: std::ops::Range<f64>,
    ) -> ChartContext<
        'a,
        SVGBackend<'b>,
        Cartesian2d<plotters::coord::types::RangedCoordf64, plotters::coord::types::RangedCoordf64>,
    > {
        let mut builder = ChartBuilder::on(root);
        builder.margin(10);

        if let Some(t) = &self.title {
            builder.caption(
                t.as_str(),
                ("sans-serif", 18).into_font().color(&COLOR_TEXT),
            );
        }
        if self.x_label.is_some() || self.y_label.is_some() {
            builder.set_label_area_size(LabelAreaPosition::Bottom, 40);
            builder.set_label_area_size(LabelAreaPosition::Left, 60);
        }

        let mut chart = builder.build_cartesian_2d(x_range, y_range).unwrap();

        let mut mesh = chart.configure_mesh();
        mesh.label_style(("sans-serif", 12).into_font().color(&COLOR_TEXT))
            .axis_style(COLOR_TEXT);

        if let Some(lbl) = &self.x_label {
            mesh.x_desc(lbl.as_str());
        }
        if let Some(lbl) = &self.y_label {
            mesh.y_desc(lbl.as_str());
        }

        mesh.draw().unwrap();

        chart
    }
}

// ── Trait impls ──────────────────────────────────────────────────────────────

impl From<Plot> for CellOutput {
    fn from(plot: Plot) -> Self {
        CellOutput::svg(plot.render_svg())
    }
}

impl IntoPanels for Plot {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Svg(self.render_svg())]
    }
}

impl TypeTag for Plot {
    fn type_tag() -> String {
        "Plot".into()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Compute x/y ranges from `(x, y)` data with a small margin so points aren't
/// clipped against the axes.
fn xy_ranges(data: &[(f64, f64)]) -> (std::ops::Range<f64>, std::ops::Range<f64>) {
    if data.is_empty() {
        return (0.0..1.0, 0.0..1.0);
    }

    let (mut x_min, mut x_max) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut y_min, mut y_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &(x, y) in data {
        x_min = x_min.min(x);
        x_max = x_max.max(x);
        y_min = y_min.min(y);
        y_max = y_max.max(y);
    }

    let x_pad = if (x_max - x_min).abs() < f64::EPSILON {
        1.0
    } else {
        (x_max - x_min) * 0.05
    };
    let y_pad = if (y_max - y_min).abs() < f64::EPSILON {
        1.0
    } else {
        (y_max - y_min) * 0.05
    };

    (
        (x_min - x_pad)..(x_max + x_pad),
        (y_min - y_pad)..(y_max + y_pad),
    )
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_produces_svg() {
        let plot = Plot::line(&[(0.0, 0.0), (1.0, 1.0), (2.0, 4.0)]);
        let svg = plot.render_svg();
        assert!(svg.contains("<svg"), "expected SVG output");
    }

    #[test]
    fn title_appears_in_svg() {
        let svg = Plot::line(&[(0.0, 0.0), (1.0, 1.0)])
            .title("My Chart")
            .render_svg();
        assert!(svg.contains("My Chart"), "title should appear in SVG");
    }

    #[test]
    fn axis_labels_appear_in_svg() {
        let svg = Plot::line(&[(0.0, 0.0), (1.0, 1.0)])
            .x_label("Time")
            .y_label("Value")
            .render_svg();
        assert!(svg.contains("Time"), "x_label should appear in SVG");
        assert!(svg.contains("Value"), "y_label should appear in SVG");
    }

    #[test]
    fn from_plot_produces_svg_cell_output() {
        let plot = Plot::line(&[(0.0, 1.0), (1.0, 2.0)]);
        let output: CellOutput = plot.into();
        let panels = output.into_panels();
        assert_eq!(panels.len(), 1);
        match &panels[0] {
            DisplayPanel::Svg(s) => assert!(s.contains("<svg")),
            other => panic!("expected Svg panel, got {other:?}"),
        }
    }

    #[test]
    fn default_size_vs_custom_size() {
        let default_svg = Plot::line(&[(0.0, 0.0), (1.0, 1.0)]).render_svg();
        let custom_svg = Plot::line(&[(0.0, 0.0), (1.0, 1.0)])
            .size(400, 200)
            .render_svg();

        assert!(
            default_svg.contains("width=\"800\""),
            "default width should be 800"
        );
        assert!(
            custom_svg.contains("width=\"400\""),
            "custom width should be 400"
        );
        assert!(
            custom_svg.contains("height=\"200\""),
            "custom height should be 200"
        );
    }

    #[test]
    fn scatter_produces_svg() {
        let svg = Plot::scatter(&[(1.0, 2.0), (3.0, 4.0)]).render_svg();
        assert!(svg.contains("<svg"), "scatter should produce SVG");
    }

    #[test]
    fn bar_produces_svg() {
        let svg = Plot::bar(&[("A", 10.0), ("B", 20.0), ("C", 15.0)]).render_svg();
        assert!(svg.contains("<svg"), "bar should produce SVG");
    }

    #[test]
    fn transparent_background() {
        let svg = Plot::line(&[(0.0, 0.0), (1.0, 1.0)]).render_svg();
        assert!(
            svg.contains("fill=\"transparent\""),
            "background should be transparent"
        );
        assert!(
            !svg.contains("fill=\"#000000\""),
            "black fill should be replaced"
        );
    }

    #[test]
    fn type_tag_is_plot() {
        assert_eq!(Plot::type_tag(), "Plot");
    }

    #[test]
    fn into_panels_produces_svg_panel() {
        let plot = Plot::scatter(&[(0.0, 0.0)]);
        let panels = plot.into_panels();
        assert_eq!(panels.len(), 1);
        assert!(matches!(panels[0], DisplayPanel::Svg(_)));
    }
}
