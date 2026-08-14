//! Ergonomic plotting API wrapping plotters' `SVGBackend`.

use std::fmt::Write as _;

use plotters::chart::MeshStyle;
use plotters::coord::types::RangedCoordf64;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};

use crate::{CellOutput, DisplayPanel, IntoPanels, TypeTag};

// ── Themed palette ──────────────────────────────────────────────────────────
//
// A cell renders in a Web Worker and its SVG is persisted into `saved_output`,
// so it cannot know the page theme at render time, and a snapshot captured
// under one theme must not stay that way forever. Nothing here bakes a theme
// colour. Every colour below is a *sentinel*: plotters draws with it, then
// `themify` rewrites it to `var(--ip-plot-NAME, <sentinel>)` and the page
// resolves it against whichever theme is active (the `--ip-plot-*` block in
// `style/main.scss`).
//
// Each sentinel doubles as the `var()` fallback, and each fallback is the
// channel-wise midpoint of that token's light and dark values. The fallback is
// load-bearing rather than decoration: every output panel carries a copy button
// and `Download .ironpad` embeds the SVG, and outside ironpad's stylesheet an
// unresolved `var()` with no fallback drops the whole attribute. A midpoint is
// the one value that reads on both grounds: measured across every text element
// of a rendered chart with no stylesheet at all, the worst case is 3.7:1 over
// the light code surface (#fbfcfe) and 4.0:1 over the dark one (#161c33), where
// the best ANY single colour can do across that pair is 4.03:1. Inside ironpad
// the tokens win and both themes clear WCAG AA outright — that is the point of
// the whole arrangement, and the fallback is only the lifeboat.
//
// Sentinels must stay pairwise distinct: `themify` keys on the emitted hex, so
// two roles sharing a value would silently collapse into one token.

const COLOR_TEXT: RGBColor = RGBColor(0x82, 0x82, 0x8C);
const COLOR_MUTED: RGBColor = RGBColor(0x79, 0x79, 0x9A);
const COLOR_GRID: RGBColor = RGBColor(0x86, 0x93, 0xAB);
const COLOR_ZERO: RGBColor = RGBColor(0x83, 0x91, 0xAC);
const COLOR_AXIS: RGBColor = RGBColor(0x70, 0x85, 0xA0);
const COLOR_SERIES_1: RGBColor = RGBColor(0xE0, 0x3F, 0x59);
const COLOR_TRANSPARENT: RGBColor = RGBColor(0, 0, 0);

/// Sentinel to token-name map, consumed by [`themify`]. The name is the
/// `--ip-plot-*` suffix declared in `style/main.scss`.
const PALETTE: &[(&str, RGBColor)] = &[
    ("text", COLOR_TEXT),
    ("muted", COLOR_MUTED),
    ("grid", COLOR_GRID),
    ("zero", COLOR_ZERO),
    ("axis", COLOR_AXIS),
    ("series-1", COLOR_SERIES_1),
];

/// Bar opacity by rank, tallest first (design handoff). Bars past the fourth
/// rank all sit at the floor.
const BAR_RANK_OPACITY: [f64; 4] = [1.0, 0.78, 0.62, 0.46];

/// Ticks per axis. The handoff caps this at 5-6; plotters treats it as a hint
/// and picks round numbers at or below it.
const MAX_TICKS: usize = 6;

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
    #[serde(default)]
    tooltips: bool,
    #[serde(default)]
    point_labels: bool,
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
            tooltips: false,
            point_labels: false,
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
            tooltips: false,
            point_labels: false,
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
            tooltips: false,
            point_labels: false,
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

    /// Enable native SVG tooltips on data points.
    #[must_use]
    pub fn tooltips(mut self, enabled: bool) -> Self {
        self.tooltips = enabled;
        self
    }

    /// Show data values as text labels on each data point.
    ///
    /// Line and scatter charts only. Bar charts always label the end of each
    /// bar, which is what the design handoff asks for in place of a dense value
    /// axis, so this flag would have nothing left to turn on for them.
    #[must_use]
    pub fn point_labels(mut self, enabled: bool) -> Self {
        self.point_labels = enabled;
        self
    }

    /// Render the chart to an SVG string.
    fn render_svg(&self) -> String {
        let mut buf = String::new();
        let mut tooltip_points: Vec<(i32, i32, String)> = Vec::new();

        {
            let root =
                SVGBackend::with_string(&mut buf, (self.width, self.height)).into_drawing_area();

            // Transparent background (writes black fill, but we strip it below).
            root.fill(&COLOR_TRANSPARENT)
                .expect("filling SVG background cannot fail");

            match &self.kind {
                ChartKind::Line(data) => self.render_line(&root, data, &mut tooltip_points),
                ChartKind::Bar(data) => self.render_bar(&root, data, &mut tooltip_points),
                ChartKind::Scatter(data) => {
                    self.render_scatter(&root, data, &mut tooltip_points);
                }
            }

            root.present().expect("presenting SVG drawing cannot fail");
        }

        // Post-process: the background rect is drawn black because plotters has
        // no transparent fill, and every palette sentinel becomes a themed
        // `var()` here (see the palette comment at the top of the module).
        let svg = themify(&buf.replace("fill=\"#000000\"", "fill=\"transparent\""));

        if self.tooltips && !tooltip_points.is_empty() {
            inject_tooltips(&svg, &tooltip_points)
        } else {
            svg
        }
    }

    // ── Per-kind renderers ───────────────────────────────────────────────

    fn render_line(
        &self,
        root: &DrawingArea<SVGBackend<'_>, plotters::coord::Shift>,
        data: &[(f64, f64)],
        tooltip_points: &mut Vec<(i32, i32, String)>,
    ) {
        let (x_range, y_range) = xy_ranges(data);

        let mut chart = self.build_chart_context(root, x_range, y_range);

        chart
            .draw_series(LineSeries::new(
                data.iter().copied(),
                COLOR_SERIES_1.stroke_width(2),
            ))
            .expect("drawing line series cannot fail");

        if self.point_labels {
            chart
                .draw_series(data.iter().map(|&(x, y)| {
                    Text::new(
                        format!("{y:.1}"),
                        (x, y),
                        ("sans-serif", 10).into_font().color(&COLOR_TEXT),
                    )
                }))
                .expect("drawing point labels cannot fail");
        }

        if self.tooltips {
            for &(x, y) in data {
                let (px, py) = chart.backend_coord(&(x, y));
                tooltip_points.push((px, py, format!("({x}, {y})")));
            }
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn render_bar(
        &self,
        root: &DrawingArea<SVGBackend<'_>, plotters::coord::Shift>,
        data: &[(String, f64)],
        tooltip_points: &mut Vec<(i32, i32, String)>,
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

        let mut chart = builder
            .build_cartesian_2d(0.0..n, 0.0..y_top)
            .expect("building bar chart context cannot fail");

        let category = |x: &f64| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let idx = *x as usize;
            data.get(idx).map_or_else(String::new, |(l, _)| l.clone())
        };

        {
            let mut mesh = chart.configure_mesh();
            apply_mesh_theme(&mut mesh);
            // One tick per category — the bar labels are the axis here, so the
            // MAX_TICKS cap does not apply to x.
            mesh.x_label_formatter(&category)
                .x_labels(data.len())
                .draw()
                .expect("drawing bar chart mesh cannot fail");
        }

        let ranks = value_ranks(data);

        chart
            .draw_series(data.iter().enumerate().map(|(i, (_, val))| {
                let x0 = i as f64 + 0.1;
                let x1 = (i + 1) as f64 - 0.1;
                let opacity = BAR_RANK_OPACITY[ranks[i].min(BAR_RANK_OPACITY.len() - 1)];
                let mut bar = Rectangle::new(
                    [(x0, 0.0), (x1, *val)],
                    COLOR_SERIES_1.mix(opacity).filled(),
                );
                bar.set_margin(0, 0, 2, 2);
                bar
            }))
            .expect("drawing bar series cannot fail");

        // Value labels at the bar end, always: the handoff trades a dense value
        // axis for these, so there is nothing for `point_labels` to toggle.
        let end_label = ("sans-serif", 11)
            .into_font()
            .color(&COLOR_TEXT)
            .pos(Pos::new(HPos::Center, VPos::Bottom));
        chart
            .draw_series(data.iter().enumerate().map(|(i, (_, val))| {
                Text::new(
                    format!("{val:.1}"),
                    (i as f64 + 0.5, *val),
                    end_label.clone(),
                )
            }))
            .expect("drawing bar value labels cannot fail");

        if self.tooltips {
            for (i, (label, val)) in data.iter().enumerate() {
                let (px, py) = chart.backend_coord(&(i as f64 + 0.5, *val));
                tooltip_points.push((px, py, format!("{label}: {val}")));
            }
        }
    }

    fn render_scatter(
        &self,
        root: &DrawingArea<SVGBackend<'_>, plotters::coord::Shift>,
        data: &[(f64, f64)],
        tooltip_points: &mut Vec<(i32, i32, String)>,
    ) {
        let (x_range, y_range) = xy_ranges(data);

        let mut chart = self.build_chart_context(root, x_range, y_range);

        chart
            .draw_series(
                data.iter()
                    .map(|(x, y)| Circle::new((*x, *y), 4, COLOR_SERIES_1.filled())),
            )
            .expect("drawing scatter series cannot fail");

        if self.point_labels {
            chart
                .draw_series(data.iter().map(|&(x, y)| {
                    Text::new(
                        format!("{y:.1}"),
                        (x, y),
                        ("sans-serif", 10).into_font().color(&COLOR_TEXT),
                    )
                }))
                .expect("drawing scatter point labels cannot fail");
        }

        if self.tooltips {
            for &(x, y) in data {
                let (px, py) = chart.backend_coord(&(x, y));
                tooltip_points.push((px, py, format!("({x}, {y})")));
            }
        }
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

        let mut chart = builder
            .build_cartesian_2d(x_range.clone(), y_range.clone())
            .expect("building chart context cannot fail");

        {
            let mut mesh = chart.configure_mesh();
            apply_mesh_theme(&mut mesh);

            if let Some(lbl) = &self.x_label {
                mesh.x_desc(lbl.as_str());
            }
            if let Some(lbl) = &self.y_label {
                mesh.y_desc(lbl.as_str());
            }

            mesh.draw().expect("drawing chart mesh cannot fail");
        }

        // A solid baseline in the zero token, per the handoff. Only when zero is
        // inside the plotted range: on a chart that never crosses it the axis
        // line already sits there and a second line on top of it just doubles
        // the stroke.
        if y_range.start < 0.0 && y_range.end > 0.0 {
            chart
                .draw_series(LineSeries::new(
                    [(x_range.start, 0.0), (x_range.end, 0.0)],
                    COLOR_ZERO.stroke_width(1),
                ))
                .expect("drawing baseline cannot fail");
        }

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

/// Shared mesh theme: horizontal gridlines only, dashed, few ticks, no chart-area
/// box, tick labels muted and axis descriptions in the text token.
///
/// The vertical mesh is off because verticals are noise on a continuous x axis,
/// and the light (subdivision) lines are drawn fully transparent, which the SVG
/// backend skips emitting altogether, so what is left is one gridline per tick.
fn apply_mesh_theme(mesh: &mut MeshStyle<'_, '_, RangedCoordf64, RangedCoordf64, SVGBackend<'_>>) {
    mesh.disable_x_mesh()
        .x_labels(MAX_TICKS)
        .y_labels(MAX_TICKS)
        .bold_line_style(COLOR_GRID)
        .light_line_style(TRANSPARENT)
        .axis_style(COLOR_AXIS)
        .label_style(("sans-serif", 12).into_font().color(&COLOR_MUTED))
        .axis_desc_style(("sans-serif", 12).into_font().color(&COLOR_TEXT));
}

/// Rank each bar by value, tallest first, so [`BAR_RANK_OPACITY`] can step down
/// the ordering rather than the input order.
fn value_ranks(data: &[(String, f64)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..data.len()).collect();
    order.sort_by(|&a, &b| data[b].1.total_cmp(&data[a].1));

    let mut ranks = vec![0; data.len()];
    for (rank, &idx) in order.iter().enumerate() {
        ranks[idx] = rank;
    }
    ranks
}

/// The hex literal plotters' SVG backend emits for a colour.
fn hex(color: RGBColor) -> String {
    format!("#{:02X}{:02X}{:02X}", color.0, color.1, color.2)
}

/// Rewrite every palette sentinel into the themed custom property that carries
/// it as a fallback.
///
/// Two roles also pick up a presentation attribute the SVG backend has no way to
/// emit, so they are rewritten in their attribute-qualified form first; the
/// generic pass below then only finds the remaining (fill) occurrences. The
/// rewritten text never contains `"#RRGGBB"` in quoted form, so a later
/// substitution cannot re-enter an earlier one.
fn themify(svg: &str) -> String {
    let mut out = svg.replace(
        &format!("stroke=\"{}\"", hex(COLOR_GRID)),
        &format!(
            "stroke=\"{}\" stroke-dasharray=\"2 5\"",
            themed("grid", COLOR_GRID)
        ),
    );
    out = out.replace(
        &format!("stroke=\"{}\"", hex(COLOR_SERIES_1)),
        &format!(
            "stroke=\"{}\" stroke-linecap=\"round\"",
            themed("series-1", COLOR_SERIES_1)
        ),
    );

    for (name, color) in PALETTE {
        out = out.replace(
            &format!("\"{}\"", hex(*color)),
            &format!("\"{}\"", themed(name, *color)),
        );
    }
    out
}

/// `var(--ip-plot-NAME, #RRGGBB)`. The fallback is never optional — see the
/// palette comment at the top of the module.
fn themed(name: &str, color: RGBColor) -> String {
    format!("var(--ip-plot-{name}, {})", hex(color))
}

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

/// Inject SVG `<title>` tooltip elements at the given pixel coordinates.
fn inject_tooltips(svg: &str, points: &[(i32, i32, String)]) -> String {
    if let Some(pos) = svg.rfind("</svg>") {
        let mut result = String::with_capacity(svg.len() + points.len() * 120);
        result.push_str(&svg[..pos]);
        result.push_str("<g class=\"ironpad-tooltips\">");
        for (px, py, label) in points {
            let escaped = xml_escape(label);
            let _ = write!(
                result,
                "<circle cx=\"{px}\" cy=\"{py}\" r=\"8\" fill=\"transparent\" stroke=\"none\">\
                 <title>{escaped}</title></circle>"
            );
        }
        result.push_str("</g></svg>");
        result
    } else {
        svg.to_owned()
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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

    #[test]
    fn tooltips_adds_title_elements() {
        let svg = Plot::scatter(&[(1.0, 2.0), (3.0, 4.0)])
            .tooltips(true)
            .render_svg();
        assert!(
            svg.contains("<title>"),
            "tooltips should add <title> elements"
        );
        assert!(
            svg.contains("ironpad-tooltips"),
            "tooltips should add ironpad-tooltips group"
        );
        assert!(
            svg.contains("(1, 2)"),
            "tooltip should contain first data point"
        );
        assert!(
            svg.contains("(3, 4)"),
            "tooltip should contain second data point"
        );
    }

    #[test]
    fn point_labels_adds_text_elements() {
        let svg = Plot::scatter(&[(1.0, 2.0), (3.0, 4.0)])
            .point_labels(true)
            .render_svg();
        assert!(svg.contains("2.0"), "point label for y=2.0 should appear");
        assert!(svg.contains("4.0"), "point label for y=4.0 should appear");
    }

    #[test]
    fn tooltips_off_by_default() {
        let svg = Plot::scatter(&[(1.0, 2.0), (3.0, 4.0)]).render_svg();
        assert!(
            !svg.contains("ironpad-tooltips"),
            "default plot should not have tooltip group"
        );
    }

    #[test]
    fn point_labels_off_by_default() {
        let svg = Plot::scatter(&[(1.0, 2.7)]).render_svg();
        assert!(
            !svg.contains("2.7"),
            "default plot should not have point label text"
        );
    }

    // ── Theming ──────────────────────────────────────────────────────────

    /// Every `var(--ip-plot-…)` occurrence in `svg`, closing paren included.
    fn theme_vars(svg: &str) -> Vec<&str> {
        svg.match_indices("var(--ip-plot-")
            .map(|(at, _)| {
                let rest = &svg[at..];
                let end = rest.find(')').expect("a var() must be closed");
                &rest[..=end]
            })
            .collect()
    }

    /// Value of `name="…"` inside a single SVG tag.
    fn attr<'a>(tag: &'a str, name: &str) -> &'a str {
        let key = format!("{name}=\"");
        let at = tag
            .find(&key)
            .unwrap_or_else(|| panic!("{name} missing from {tag}"))
            + key.len();
        let rest = &tag[at..];
        &rest[..rest.find('"').expect("attribute must be closed")]
    }

    fn themed_chart() -> String {
        Plot::line(&[(0.0, -1.0), (1.0, 1.0), (2.0, 4.0)])
            .title("Themed")
            .x_label("x")
            .y_label("y")
            .render_svg()
    }

    #[test]
    fn text_and_series_render_as_theme_variables() {
        let svg = themed_chart();
        for token in [
            "var(--ip-plot-text,",
            "var(--ip-plot-muted,",
            "var(--ip-plot-grid,",
            "var(--ip-plot-zero,",
            "var(--ip-plot-axis,",
            "var(--ip-plot-series-1,",
        ] {
            assert!(svg.contains(token), "{token} missing from rendered SVG");
        }
    }

    #[test]
    fn every_theme_var_carries_a_fallback() {
        // Outside ironpad's stylesheet — the clipboard, a downloaded notebook —
        // an unresolved var() with no fallback drops the attribute entirely.
        let svg = themed_chart();
        let vars = theme_vars(&svg);
        assert!(!vars.is_empty(), "expected themed colours in the SVG");
        for v in vars {
            assert!(v.contains(", #"), "{v} has no fallback colour");
        }
    }

    #[test]
    fn no_palette_hex_survives_the_post_process() {
        let svg = themed_chart();
        for (name, color) in PALETTE {
            let bare = format!("\"{}\"", hex(*color));
            assert!(
                !svg.contains(&bare),
                "{name} still emitted as a bare {bare} attribute value"
            );
        }
        // The pre-PRD-0065 dark palette, in case a call site is ever missed.
        assert!(
            !svg.contains("#EAEAEA"),
            "the old baked text colour survives"
        );
        assert!(
            !svg.contains("#E94560"),
            "the old baked accent colour survives"
        );

        // Nothing at all may reach the page as a raw literal. plotters supplies
        // its own defaults the moment a style is left unset (black at low alpha
        // for the subdivision gridlines, for one), and those are invisible in
        // one theme and wrong in the other while carrying no palette hex to
        // search for.
        for svg in [svg, Plot::bar(&[("A", 1.0)]).tooltips(true).render_svg()] {
            assert!(
                !svg.contains("=\"#"),
                "an unthemed colour literal reaches the page"
            );
        }
    }

    #[test]
    fn sentinels_are_pairwise_distinct() {
        // themify keys on the emitted hex, so a duplicate would silently merge
        // two roles into one token.
        for (i, (name_a, a)) in PALETTE.iter().enumerate() {
            for (name_b, b) in &PALETTE[i + 1..] {
                assert_ne!(hex(*a), hex(*b), "{name_a} and {name_b} share a sentinel");
            }
        }
    }

    #[test]
    fn gridlines_are_horizontal_dashed_and_capped() {
        let svg = themed_chart();
        let grid: Vec<&str> = svg
            .lines()
            .filter(|l| l.contains("var(--ip-plot-grid,"))
            .collect();

        assert!(!grid.is_empty(), "expected gridlines");
        const { assert!(MAX_TICKS <= 6, "the handoff caps an axis at 5-6 ticks") };
        assert!(
            grid.len() <= MAX_TICKS,
            "{} gridlines exceeds the {MAX_TICKS}-tick cap",
            grid.len()
        );
        for line in grid {
            assert!(
                line.contains("stroke-dasharray=\"2 5\""),
                "gridline is not dashed: {line}"
            );
            assert_eq!(
                attr(line, "y1"),
                attr(line, "y2"),
                "vertical gridlines are off by default: {line}"
            );
        }
    }

    #[test]
    fn series_stroke_is_round_capped() {
        let svg = themed_chart();
        let series = svg
            .lines()
            .find(|l| l.contains("var(--ip-plot-series-1,") && l.contains("stroke="))
            .expect("expected a stroked series");
        assert!(
            series.contains("stroke-linecap=\"round\""),
            "series stroke is not round-capped: {series}"
        );
    }

    #[test]
    fn baseline_drawn_only_when_zero_is_inside_the_range() {
        let crossing = Plot::line(&[(0.0, -1.0), (1.0, 1.0)]).render_svg();
        assert!(
            crossing.contains("var(--ip-plot-zero,"),
            "a chart crossing zero should carry a baseline"
        );

        let above = Plot::line(&[(0.0, 1.0), (1.0, 4.0)]).render_svg();
        assert!(
            !above.contains("var(--ip-plot-zero,"),
            "a chart that never crosses zero already has the axis there"
        );
    }

    #[test]
    fn bars_label_their_ends_and_ramp_opacity_by_rank() {
        let svg = Plot::bar(&[("A", 3.7), ("B", 9.1), ("C", 5.3), ("D", 1.2)]).render_svg();

        // Labels are unconditional for bars — no `.point_labels(true)` above.
        for value in ["3.7", "9.1", "5.3", "1.2"] {
            assert!(svg.contains(value), "bar end label {value} missing");
        }

        let opacities: Vec<&str> = svg
            .lines()
            .filter(|l| l.starts_with("<rect") && l.contains("var(--ip-plot-series-1,"))
            .map(|l| attr(l, "opacity"))
            .collect();
        // Input order A, B, C, D against ranks 2, 0, 1, 3 by value.
        assert_eq!(opacities, ["0.62", "1", "0.78", "0.46"]);
    }
}
