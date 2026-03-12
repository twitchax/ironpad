//! Interactive UI widget builders for cell output.
//!
//! Each builder produces a [`CellOutput`] with a serialized default value and
//! an [`DisplayPanel::Interactive`] panel describing the widget for the
//! frontend renderer.
//!
//! # Examples
//!
//! ```ignore
//! ui::slider(1.0, 10.0).step(0.5).label("Speed")
//! ui::dropdown(&["red", "green", "blue"]).label("Color")
//! ui::checkbox("Enable logging")
//! ```

use crate::{CellOutput, DisplayPanel, IntoPanels, TypeTag};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn encode_bincode<T: serde::Serialize>(value: &T) -> Vec<u8> {
    bincode::serde::encode_to_vec(value, bincode::config::standard())
        .expect("serialization of primitive widget value cannot fail")
}

fn simple_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!("{:08x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

// ── Slider ───────────────────────────────────────────────────────────────────

/// Builder for a range slider widget producing an `f64` value.
pub struct Slider {
    min: f64,
    max: f64,
    step: f64,
    label: Option<String>,
    default: f64,
}

/// Create a slider widget with the given range.
#[must_use]
pub fn slider(min: f64, max: f64) -> Slider {
    Slider {
        min,
        max,
        step: 1.0,
        label: None,
        default: min,
    }
}

impl Slider {
    /// Set the step increment.
    #[must_use]
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// Set an optional label.
    #[must_use]
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_owned());
        self
    }

    /// Override the default value.
    #[must_use]
    pub fn default_value(mut self, value: f64) -> Self {
        self.default = value;
        self
    }

    fn config_json(&self) -> String {
        serde_json::json!({
            "min": self.min,
            "max": self.max,
            "step": self.step,
            "label": self.label,
            "default": self.default,
        })
        .to_string()
    }
}

impl From<Slider> for CellOutput {
    fn from(s: Slider) -> Self {
        Self {
            bytes: encode_bincode(&s.default),
            panels: vec![DisplayPanel::Interactive {
                kind: "slider".into(),
                config: s.config_json(),
            }],
            type_tag: Some("f64".into()),
        }
    }
}

impl IntoPanels for Slider {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Interactive {
            kind: "slider".into(),
            config: self.config_json(),
        }]
    }
}

impl TypeTag for Slider {
    fn type_tag() -> String {
        "f64".into()
    }
}

impl serde::Serialize for Slider {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.default.serialize(serializer)
    }
}

// ── Dropdown ─────────────────────────────────────────────────────────────────

/// Builder for a dropdown select widget producing a `String` value.
pub struct Dropdown {
    options: Vec<String>,
    label: Option<String>,
    default: String,
}

/// Create a dropdown widget with the given options.
#[must_use]
pub fn dropdown(options: &[&str]) -> Dropdown {
    let options: Vec<String> = options.iter().map(|s| (*s).to_owned()).collect();
    let default = options.first().cloned().unwrap_or_default();
    Dropdown {
        options,
        label: None,
        default,
    }
}

impl Dropdown {
    /// Set an optional label.
    #[must_use]
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_owned());
        self
    }

    /// Override the default selected value.
    #[must_use]
    pub fn default_value(mut self, value: &str) -> Self {
        self.default = value.to_owned();
        self
    }

    fn config_json(&self) -> String {
        serde_json::json!({
            "options": self.options,
            "label": self.label,
            "default": self.default,
        })
        .to_string()
    }
}

impl From<Dropdown> for CellOutput {
    fn from(d: Dropdown) -> Self {
        Self {
            bytes: encode_bincode(&d.default),
            panels: vec![DisplayPanel::Interactive {
                kind: "dropdown".into(),
                config: d.config_json(),
            }],
            type_tag: Some("String".into()),
        }
    }
}

impl IntoPanels for Dropdown {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Interactive {
            kind: "dropdown".into(),
            config: self.config_json(),
        }]
    }
}

impl TypeTag for Dropdown {
    fn type_tag() -> String {
        "String".into()
    }
}

impl serde::Serialize for Dropdown {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.default.serialize(serializer)
    }
}

// ── Checkbox ─────────────────────────────────────────────────────────────────

/// Builder for a checkbox widget producing a `bool` value.
pub struct Checkbox {
    label: String,
    default: bool,
}

/// Create a checkbox widget with the given label.
#[must_use]
pub fn checkbox(label: &str) -> Checkbox {
    Checkbox {
        label: label.to_owned(),
        default: false,
    }
}

impl Checkbox {
    /// Override the default checked state.
    #[must_use]
    pub fn default_value(mut self, value: bool) -> Self {
        self.default = value;
        self
    }

    fn config_json(&self) -> String {
        serde_json::json!({
            "label": self.label,
            "default": self.default,
        })
        .to_string()
    }
}

impl From<Checkbox> for CellOutput {
    fn from(c: Checkbox) -> Self {
        Self {
            bytes: encode_bincode(&c.default),
            panels: vec![DisplayPanel::Interactive {
                kind: "checkbox".into(),
                config: c.config_json(),
            }],
            type_tag: Some("bool".into()),
        }
    }
}

impl IntoPanels for Checkbox {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Interactive {
            kind: "checkbox".into(),
            config: self.config_json(),
        }]
    }
}

impl TypeTag for Checkbox {
    fn type_tag() -> String {
        "bool".into()
    }
}

impl serde::Serialize for Checkbox {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.default.serialize(serializer)
    }
}

// ── TextInput ────────────────────────────────────────────────────────────────

/// Builder for a text input widget producing a `String` value.
pub struct TextInput {
    placeholder: String,
    label: Option<String>,
    default: String,
}

/// Create a text input widget with the given placeholder.
#[must_use]
pub fn text_input(placeholder: &str) -> TextInput {
    TextInput {
        placeholder: placeholder.to_owned(),
        label: None,
        default: String::new(),
    }
}

impl TextInput {
    /// Set an optional label.
    #[must_use]
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_owned());
        self
    }

    /// Override the default text value.
    #[must_use]
    pub fn default_value(mut self, value: &str) -> Self {
        self.default = value.to_owned();
        self
    }

    fn config_json(&self) -> String {
        serde_json::json!({
            "placeholder": self.placeholder,
            "label": self.label,
            "default": self.default,
        })
        .to_string()
    }
}

impl From<TextInput> for CellOutput {
    fn from(t: TextInput) -> Self {
        Self {
            bytes: encode_bincode(&t.default),
            panels: vec![DisplayPanel::Interactive {
                kind: "text_input".into(),
                config: t.config_json(),
            }],
            type_tag: Some("String".into()),
        }
    }
}

impl IntoPanels for TextInput {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Interactive {
            kind: "text_input".into(),
            config: self.config_json(),
        }]
    }
}

impl TypeTag for TextInput {
    fn type_tag() -> String {
        "String".into()
    }
}

impl serde::Serialize for TextInput {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.default.serialize(serializer)
    }
}

// ── Number ───────────────────────────────────────────────────────────────────

/// Builder for a number input widget producing an `f64` value.
pub struct Number {
    min: f64,
    max: f64,
    step: f64,
    label: Option<String>,
    default: f64,
}

/// Create a number input widget with the given range.
#[must_use]
pub fn number(min: f64, max: f64) -> Number {
    Number {
        min,
        max,
        step: 1.0,
        label: None,
        default: min,
    }
}

impl Number {
    /// Set the step increment.
    #[must_use]
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// Set an optional label.
    #[must_use]
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_owned());
        self
    }

    /// Override the default value.
    #[must_use]
    pub fn default_value(mut self, value: f64) -> Self {
        self.default = value;
        self
    }

    fn config_json(&self) -> String {
        serde_json::json!({
            "min": self.min,
            "max": self.max,
            "step": self.step,
            "label": self.label,
            "default": self.default,
        })
        .to_string()
    }
}

impl From<Number> for CellOutput {
    fn from(n: Number) -> Self {
        Self {
            bytes: encode_bincode(&n.default),
            panels: vec![DisplayPanel::Interactive {
                kind: "number".into(),
                config: n.config_json(),
            }],
            type_tag: Some("f64".into()),
        }
    }
}

impl IntoPanels for Number {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Interactive {
            kind: "number".into(),
            config: self.config_json(),
        }]
    }
}

impl TypeTag for Number {
    fn type_tag() -> String {
        "f64".into()
    }
}

impl serde::Serialize for Number {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.default.serialize(serializer)
    }
}

// ── Switch ───────────────────────────────────────────────────────────────────

/// Builder for a toggle switch widget producing a `bool` value.
pub struct Switch {
    label: String,
    default: bool,
}

/// Create a toggle switch widget with the given label.
#[must_use]
pub fn switch(label: &str) -> Switch {
    Switch {
        label: label.to_owned(),
        default: false,
    }
}

impl Switch {
    /// Override the default state.
    #[must_use]
    pub fn default_value(mut self, value: bool) -> Self {
        self.default = value;
        self
    }

    fn config_json(&self) -> String {
        serde_json::json!({
            "label": self.label,
            "default": self.default,
        })
        .to_string()
    }
}

impl From<Switch> for CellOutput {
    fn from(s: Switch) -> Self {
        Self {
            bytes: encode_bincode(&s.default),
            panels: vec![DisplayPanel::Interactive {
                kind: "switch".into(),
                config: s.config_json(),
            }],
            type_tag: Some("bool".into()),
        }
    }
}

impl IntoPanels for Switch {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Interactive {
            kind: "switch".into(),
            config: self.config_json(),
        }]
    }
}

impl TypeTag for Switch {
    fn type_tag() -> String {
        "bool".into()
    }
}

impl serde::Serialize for Switch {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.default.serialize(serializer)
    }
}

// ── Button ───────────────────────────────────────────────────────────────────

/// Builder for a button widget that acts as a trigger (no data output).
pub struct Button {
    label: String,
}

/// Create a button widget with the given label.
#[must_use]
pub fn button(label: &str) -> Button {
    Button {
        label: label.to_owned(),
    }
}

impl Button {
    fn config_json(&self) -> String {
        serde_json::json!({
            "label": self.label,
        })
        .to_string()
    }
}

impl From<Button> for CellOutput {
    fn from(b: Button) -> Self {
        Self {
            bytes: Vec::new(),
            panels: vec![DisplayPanel::Interactive {
                kind: "button".into(),
                config: b.config_json(),
            }],
            type_tag: Some("()".into()),
        }
    }
}

impl IntoPanels for Button {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Interactive {
            kind: "button".into(),
            config: self.config_json(),
        }]
    }
}

impl TypeTag for Button {
    fn type_tag() -> String {
        "()".into()
    }
}

impl serde::Serialize for Button {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ().serialize(serializer)
    }
}

// ── ProgressBar ──────────────────────────────────────────────────────────────

/// Builder for a progress bar widget with real-time update support.
pub struct ProgressBar {
    label: Option<String>,
    initial: f64,
    id: String,
}

/// Create a progress bar widget.
#[must_use]
pub fn progress_bar() -> ProgressBar {
    ProgressBar {
        label: None,
        initial: 0.0,
        id: format!("progress-{}", simple_id()),
    }
}

impl ProgressBar {
    /// Set an optional label.
    #[must_use]
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_owned());
        self
    }

    /// Set the initial progress value (0.0 to 100.0).
    #[must_use]
    pub fn initial(mut self, value: f64) -> Self {
        self.initial = value.clamp(0.0, 100.0);
        self
    }

    fn config_json(&self) -> String {
        serde_json::json!({
            "label": self.label,
            "initial": self.initial,
            "id": self.id,
        })
        .to_string()
    }
}

impl From<ProgressBar> for CellOutput {
    fn from(pb: ProgressBar) -> Self {
        Self {
            bytes: encode_bincode(&pb.id),
            panels: vec![DisplayPanel::Interactive {
                kind: "progress".into(),
                config: pb.config_json(),
            }],
            type_tag: Some("String".into()),
        }
    }
}

impl IntoPanels for ProgressBar {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Interactive {
            kind: "progress".into(),
            config: self.config_json(),
        }]
    }
}

impl TypeTag for ProgressBar {
    fn type_tag() -> String {
        "String".into()
    }
}

impl serde::Serialize for ProgressBar {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.id.serialize(serializer)
    }
}

// ── ProgressHandle ──────────────────────────────────────────────────────────

/// Handle for updating a progress bar from a downstream cell.
///
/// When a cell outputs a `ProgressBar`, downstream cells receive
/// the progress bar's ID as a `String`. Wrap it in a `ProgressHandle`
/// to send real-time updates:
///
/// ```ignore
/// let handle = ProgressHandle::new(cell0.deserialize::<String>().unwrap());
/// handle.update(50.0); // Set to 50%
/// ```
pub struct ProgressHandle {
    id: String,
}

impl ProgressHandle {
    /// Create a handle from a progress bar ID (obtained from upstream cell output).
    pub fn new(id: String) -> Self {
        Self { id }
    }

    /// Update the progress bar to the given percentage (0.0–100.0).
    pub fn update(&self, value: f64) {
        let clamped = value.clamp(0.0, 100.0);
        let msg = serde_json::json!({
            "type": "progress_update",
            "id": self.id,
            "value": clamped,
        });
        crate::host_message_json(&msg);
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode an `f64` from bincode bytes.
    fn decode_f64(bytes: &[u8]) -> f64 {
        let (val, _): (f64, _) =
            bincode::serde::decode_from_slice(bytes, bincode::config::standard())
                .expect("decode f64");
        val
    }

    /// Decode a `String` from bincode bytes.
    fn decode_string(bytes: &[u8]) -> String {
        let (val, _): (String, _) =
            bincode::serde::decode_from_slice(bytes, bincode::config::standard())
                .expect("decode String");
        val
    }

    /// Decode a `bool` from bincode bytes.
    fn decode_bool(bytes: &[u8]) -> bool {
        let (val, _): (bool, _) =
            bincode::serde::decode_from_slice(bytes, bincode::config::standard())
                .expect("decode bool");
        val
    }

    /// Assert the output has exactly one Interactive panel with the given kind.
    fn assert_interactive(output: &CellOutput, expected_kind: &str) -> serde_json::Value {
        assert_eq!(output.panels.len(), 1);
        match &output.panels[0] {
            DisplayPanel::Interactive { kind, config } => {
                assert_eq!(kind, expected_kind);
                serde_json::from_str(config).expect("config should be valid JSON")
            }
            other => panic!("expected Interactive panel, got {other:?}"),
        }
    }

    // ── Slider tests ─────────────────────────────────────────────────────

    #[test]
    fn slider_default_value() {
        let output: CellOutput = slider(1.0, 10.0).into();
        assert!((decode_f64(&output.bytes) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn slider_type_tag() {
        let output: CellOutput = slider(0.0, 1.0).into();
        assert_eq!(output.type_tag.as_deref(), Some("f64"));
        assert_eq!(Slider::type_tag(), "f64");
    }

    #[test]
    fn slider_panel_and_config() {
        let output: CellOutput = slider(0.0, 100.0).step(5.0).label("Volume").into();
        let cfg = assert_interactive(&output, "slider");
        assert!((cfg["min"].as_f64().unwrap()).abs() < f64::EPSILON);
        assert!((cfg["max"].as_f64().unwrap() - 100.0).abs() < f64::EPSILON);
        assert!((cfg["step"].as_f64().unwrap() - 5.0).abs() < f64::EPSILON);
        assert_eq!(cfg["label"].as_str().unwrap(), "Volume");
        assert!((cfg["default"].as_f64().unwrap()).abs() < f64::EPSILON);
    }

    #[test]
    fn slider_custom_default() {
        let output: CellOutput = slider(1.0, 10.0).default_value(5.0).into();
        assert!((decode_f64(&output.bytes) - 5.0).abs() < f64::EPSILON);
        let cfg = assert_interactive(&output, "slider");
        assert!((cfg["default"].as_f64().unwrap() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn slider_label_null_by_default() {
        let output: CellOutput = slider(0.0, 1.0).into();
        let cfg = assert_interactive(&output, "slider");
        assert!(cfg["label"].is_null());
    }

    #[test]
    fn slider_into_panels() {
        let s = slider(0.0, 10.0);
        let panels = s.into_panels();
        assert_eq!(panels.len(), 1);
        assert!(matches!(&panels[0], DisplayPanel::Interactive { kind, .. } if kind == "slider"));
    }

    // ── Dropdown tests ───────────────────────────────────────────────────

    #[test]
    fn dropdown_default_value() {
        let output: CellOutput = dropdown(&["red", "green", "blue"]).into();
        assert_eq!(decode_string(&output.bytes), "red");
    }

    #[test]
    fn dropdown_type_tag() {
        let output: CellOutput = dropdown(&["a"]).into();
        assert_eq!(output.type_tag.as_deref(), Some("String"));
        assert_eq!(Dropdown::type_tag(), "String");
    }

    #[test]
    fn dropdown_panel_and_config() {
        let output: CellOutput = dropdown(&["a", "b", "c"]).label("Pick one").into();
        let cfg = assert_interactive(&output, "dropdown");
        let opts: Vec<&str> = cfg["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(opts, vec!["a", "b", "c"]);
        assert_eq!(cfg["label"].as_str().unwrap(), "Pick one");
        assert_eq!(cfg["default"].as_str().unwrap(), "a");
    }

    #[test]
    fn dropdown_custom_default() {
        let output: CellOutput = dropdown(&["x", "y", "z"]).default_value("z").into();
        assert_eq!(decode_string(&output.bytes), "z");
        let cfg = assert_interactive(&output, "dropdown");
        assert_eq!(cfg["default"].as_str().unwrap(), "z");
    }

    #[test]
    fn dropdown_empty_options() {
        let output: CellOutput = dropdown(&[]).into();
        assert_eq!(decode_string(&output.bytes), "");
        let cfg = assert_interactive(&output, "dropdown");
        assert!(cfg["options"].as_array().unwrap().is_empty());
    }

    #[test]
    fn dropdown_into_panels() {
        let d = dropdown(&["a"]);
        let panels = d.into_panels();
        assert_eq!(panels.len(), 1);
        assert!(matches!(&panels[0], DisplayPanel::Interactive { kind, .. } if kind == "dropdown"));
    }

    // ── Checkbox tests ───────────────────────────────────────────────────

    #[test]
    fn checkbox_default_value() {
        let output: CellOutput = checkbox("Enable").into();
        assert!(!decode_bool(&output.bytes));
    }

    #[test]
    fn checkbox_type_tag() {
        let output: CellOutput = checkbox("x").into();
        assert_eq!(output.type_tag.as_deref(), Some("bool"));
        assert_eq!(Checkbox::type_tag(), "bool");
    }

    #[test]
    fn checkbox_panel_and_config() {
        let output: CellOutput = checkbox("Debug mode").into();
        let cfg = assert_interactive(&output, "checkbox");
        assert_eq!(cfg["label"].as_str().unwrap(), "Debug mode");
        assert!(!cfg["default"].as_bool().unwrap());
    }

    #[test]
    fn checkbox_custom_default() {
        let output: CellOutput = checkbox("On").default_value(true).into();
        assert!(decode_bool(&output.bytes));
        let cfg = assert_interactive(&output, "checkbox");
        assert!(cfg["default"].as_bool().unwrap());
    }

    #[test]
    fn checkbox_into_panels() {
        let c = checkbox("x");
        let panels = c.into_panels();
        assert_eq!(panels.len(), 1);
        assert!(matches!(&panels[0], DisplayPanel::Interactive { kind, .. } if kind == "checkbox"));
    }

    // ── TextInput tests ──────────────────────────────────────────────────

    #[test]
    fn text_input_default_value() {
        let output: CellOutput = text_input("Enter name...").into();
        assert_eq!(decode_string(&output.bytes), "");
    }

    #[test]
    fn text_input_type_tag() {
        let output: CellOutput = text_input("").into();
        assert_eq!(output.type_tag.as_deref(), Some("String"));
        assert_eq!(TextInput::type_tag(), "String");
    }

    #[test]
    fn text_input_panel_and_config() {
        let output: CellOutput = text_input("Search...").label("Query").into();
        let cfg = assert_interactive(&output, "text_input");
        assert_eq!(cfg["placeholder"].as_str().unwrap(), "Search...");
        assert_eq!(cfg["label"].as_str().unwrap(), "Query");
        assert_eq!(cfg["default"].as_str().unwrap(), "");
    }

    #[test]
    fn text_input_custom_default() {
        let output: CellOutput = text_input("hint").default_value("hello").into();
        assert_eq!(decode_string(&output.bytes), "hello");
        let cfg = assert_interactive(&output, "text_input");
        assert_eq!(cfg["default"].as_str().unwrap(), "hello");
    }

    #[test]
    fn text_input_into_panels() {
        let t = text_input("x");
        let panels = t.into_panels();
        assert_eq!(panels.len(), 1);
        assert!(
            matches!(&panels[0], DisplayPanel::Interactive { kind, .. } if kind == "text_input")
        );
    }

    // ── Number tests ─────────────────────────────────────────────────────

    #[test]
    fn number_default_value() {
        let output: CellOutput = number(0.0, 100.0).into();
        assert!(decode_f64(&output.bytes).abs() < f64::EPSILON);
    }

    #[test]
    fn number_type_tag() {
        let output: CellOutput = number(0.0, 1.0).into();
        assert_eq!(output.type_tag.as_deref(), Some("f64"));
        assert_eq!(Number::type_tag(), "f64");
    }

    #[test]
    fn number_panel_and_config() {
        let output: CellOutput = number(0.0, 50.0).step(0.5).label("Count").into();
        let cfg = assert_interactive(&output, "number");
        assert!((cfg["min"].as_f64().unwrap()).abs() < f64::EPSILON);
        assert!((cfg["max"].as_f64().unwrap() - 50.0).abs() < f64::EPSILON);
        assert!((cfg["step"].as_f64().unwrap() - 0.5).abs() < f64::EPSILON);
        assert_eq!(cfg["label"].as_str().unwrap(), "Count");
    }

    #[test]
    fn number_custom_default() {
        let output: CellOutput = number(1.0, 10.0).default_value(7.0).into();
        assert!((decode_f64(&output.bytes) - 7.0).abs() < f64::EPSILON);
        let cfg = assert_interactive(&output, "number");
        assert!((cfg["default"].as_f64().unwrap() - 7.0).abs() < f64::EPSILON);
    }

    #[test]
    fn number_into_panels() {
        let n = number(0.0, 1.0);
        let panels = n.into_panels();
        assert_eq!(panels.len(), 1);
        assert!(matches!(&panels[0], DisplayPanel::Interactive { kind, .. } if kind == "number"));
    }

    // ── Switch tests ─────────────────────────────────────────────────────

    #[test]
    fn switch_default_value() {
        let output: CellOutput = switch("Dark mode").into();
        assert!(!decode_bool(&output.bytes));
    }

    #[test]
    fn switch_type_tag() {
        let output: CellOutput = switch("x").into();
        assert_eq!(output.type_tag.as_deref(), Some("bool"));
        assert_eq!(Switch::type_tag(), "bool");
    }

    #[test]
    fn switch_panel_and_config() {
        let output: CellOutput = switch("Turbo").into();
        let cfg = assert_interactive(&output, "switch");
        assert_eq!(cfg["label"].as_str().unwrap(), "Turbo");
        assert!(!cfg["default"].as_bool().unwrap());
    }

    #[test]
    fn switch_custom_default() {
        let output: CellOutput = switch("On").default_value(true).into();
        assert!(decode_bool(&output.bytes));
        let cfg = assert_interactive(&output, "switch");
        assert!(cfg["default"].as_bool().unwrap());
    }

    #[test]
    fn switch_into_panels() {
        let s = switch("x");
        let panels = s.into_panels();
        assert_eq!(panels.len(), 1);
        assert!(matches!(&panels[0], DisplayPanel::Interactive { kind, .. } if kind == "switch"));
    }

    // ── Button tests ─────────────────────────────────────────────────────

    #[test]
    fn button_produces_interactive_panel() {
        let output: CellOutput = button("Run").into();
        let cfg = assert_interactive(&output, "button");
        assert_eq!(cfg["label"].as_str().unwrap(), "Run");
    }

    #[test]
    fn button_type_tag() {
        let output: CellOutput = button("Go").into();
        assert_eq!(output.type_tag.as_deref(), Some("()"));
        assert_eq!(Button::type_tag(), "()");
    }

    #[test]
    fn button_empty_bytes() {
        let output: CellOutput = button("Click").into();
        assert!(output.bytes.is_empty());
    }

    #[test]
    fn button_into_panels() {
        let b = button("x");
        let panels = b.into_panels();
        assert_eq!(panels.len(), 1);
        assert!(matches!(&panels[0], DisplayPanel::Interactive { kind, .. } if kind == "button"));
    }

    // ── Cross-cutting tests ──────────────────────────────────────────────

    #[test]
    fn interactive_panel_serializes_to_json() {
        let panel = DisplayPanel::Interactive {
            kind: "slider".into(),
            config: r#"{"min":0.0}"#.into(),
        };
        let json = serde_json::to_string(&panel).expect("serialize");
        let decoded: DisplayPanel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(panel, decoded);
    }

    #[test]
    fn slider_full_chain() {
        let output: CellOutput = slider(0.0, 100.0)
            .step(0.1)
            .label("Brightness")
            .default_value(50.0)
            .into();
        assert!((decode_f64(&output.bytes) - 50.0).abs() < f64::EPSILON);
        assert_eq!(output.type_tag.as_deref(), Some("f64"));
        let cfg = assert_interactive(&output, "slider");
        assert!((cfg["step"].as_f64().unwrap() - 0.1).abs() < f64::EPSILON);
        assert_eq!(cfg["label"].as_str().unwrap(), "Brightness");
    }

    #[test]
    fn number_full_chain() {
        let output: CellOutput = number(-10.0, 10.0)
            .step(0.01)
            .label("Offset")
            .default_value(0.0)
            .into();
        assert!(decode_f64(&output.bytes).abs() < f64::EPSILON);
        assert_eq!(output.type_tag.as_deref(), Some("f64"));
        let cfg = assert_interactive(&output, "number");
        assert!((cfg["min"].as_f64().unwrap() - (-10.0)).abs() < f64::EPSILON);
    }

    // ── Serialize / tuple tests ──────────────────────────────────────────

    #[test]
    fn slider_serialize_matches_from_bytes() {
        let s = slider(0.0, 100.0).default_value(42.0);
        let expected = encode_bincode(&s.default);
        let actual = encode_bincode(&s);
        assert_eq!(actual, expected);
    }

    #[test]
    fn tuple_slider_button_produces_valid_cell_output() {
        let s = slider(0.0, 100.0).default_value(25.0);
        let b = button("Go");
        let output = CellOutput::from((s, b));

        assert_eq!(output.type_tag.as_deref(), Some("(f64, ())"));

        let (decoded, _): ((f64, ()), _) =
            bincode::serde::decode_from_slice(&output.bytes, bincode::config::standard())
                .expect("decode (f64, ())");
        assert!((decoded.0 - 25.0).abs() < f64::EPSILON);
        assert_eq!(decoded.1, ());
    }

    #[test]
    fn tuple_slider_dropdown_produces_valid_cell_output() {
        let s = slider(0.0, 10.0).default_value(3.0);
        let d = dropdown(&["red", "green", "blue"]);
        let output = CellOutput::from((s, d));

        assert_eq!(output.type_tag.as_deref(), Some("(f64, String)"));

        let (decoded, _): ((f64, String), _) =
            bincode::serde::decode_from_slice(&output.bytes, bincode::config::standard())
                .expect("decode (f64, String)");
        assert!((decoded.0 - 3.0).abs() < f64::EPSILON);
        assert_eq!(decoded.1, "red");
    }

    // ── ProgressBar tests ────────────────────────────────────────────────

    #[test]
    fn progress_bar_produces_interactive_panel() {
        let output: CellOutput = progress_bar().label("Loading").into();
        let cfg = assert_interactive(&output, "progress");
        assert_eq!(cfg["label"].as_str().unwrap(), "Loading");
        assert!(cfg["id"].as_str().unwrap().starts_with("progress-"));
    }

    #[test]
    fn progress_bar_id_is_unique() {
        let a = progress_bar();
        let b = progress_bar();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn progress_bar_serializes_as_id() {
        let pb = progress_bar();
        let expected_id = pb.id.clone();
        let json = serde_json::to_string(&pb).expect("serialize");
        let decoded: String = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, expected_id);
    }

    #[test]
    fn progress_handle_update_noop_on_native() {
        let handle = ProgressHandle::new("progress-test".to_owned());
        // Should not panic on native (non-WASM) — host_message is a no-op.
        handle.update(50.0);
    }
}
