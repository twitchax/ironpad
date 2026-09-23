//! Contract tests between the cell runtime (`ironpad-cell`) and the app.
//!
//! The app mirrors the runtime's wire types instead of depending on the crate
//! (`DisplayPanel`, `SimSliderMeta`, the `CellInputs` encoder, the widget
//! kinds and the live-view kind codes), which keeps the UI free of the cell
//! runtime but leaves only comments holding the two copies together. A variant
//! or field added on one side then decodes as a parse failure on the other at
//! runtime, where no compile error can reach it. These tests link the real
//! runtime as a dev-dependency and hold each mirror to it.

use std::collections::BTreeSet;

use ironpad_cell::{
    ui, CellInputs, CellOutput, IntoPanels, LiveContent, LiveTickResult, LiveViewMeta,
    SimSliderMeta,
};

use crate::components::executor::encode_cell_inputs;
use crate::components::live_view_panel::live_kind_str;
use crate::components::output_render::{DisplayPanel, WIDGET_KINDS};

/// One of every `ironpad_cell::DisplayPanel` variant, with every field set to
/// something distinguishable from its default.
fn every_cell_panel() -> Vec<ironpad_cell::DisplayPanel> {
    use ironpad_cell::DisplayPanel as P;
    vec![
        P::Text("text".into()),
        P::Html("<b>html</b>".into()),
        P::Svg("<svg/>".into()),
        P::Markdown("# md".into()),
        P::Table {
            headers: vec!["h".into()],
            rows: vec![vec!["r".into()]],
        },
        P::Interactive {
            kind: "slider".into(),
            config: r#"{"min":0}"#.into(),
        },
        P::BlobImage {
            mime_type: "image/bmp".into(),
            base64_data: "Qk0=".into(),
            width: 3,
            height: 2,
        },
        P::Animation {
            width: 4,
            height: 5,
            fps: 12,
            frame_count: 2,
            data: "AAAA".into(),
        },
        P::Simulation {
            width: 6,
            height: 7,
            fps: 30,
            first_frame_data: "BBBB".into(),
            sliders: vec![SimSliderMeta {
                key: "speed".into(),
                min: 0.5,
                max: 9.5,
                step: 0.25,
                label: "Speed".into(),
                default: 1.5,
            }],
        },
        P::LiveView {
            fps: 10,
            kind: "markdown".into(),
            content: "live".into(),
        },
    ]
}

#[test]
fn every_cell_panel_decodes_as_the_app_mirror() {
    for panel in every_cell_panel() {
        let json = serde_json::to_value(&panel).expect("cell panel serializes");
        let mirrored: DisplayPanel = serde_json::from_value(json.clone())
            .unwrap_or_else(|e| panic!("app mirror cannot decode {json}: {e}"));
        // Re-serializing catches the other direction: a field the cell emits
        // that the mirror silently ignores would vanish here.
        assert_eq!(
            serde_json::to_value(&mirrored).expect("app panel serializes"),
            json,
            "app mirror does not round-trip the cell's panel"
        );
    }
}

#[test]
fn encode_cell_inputs_matches_cell_decoder() {
    let outputs = [b"a".as_slice(), b"", b"xyz"];
    let encoded = encode_cell_inputs(&outputs);

    assert_eq!(
        encoded,
        CellInputs::serialize(&outputs),
        "app encoder and cell encoder disagree on the wire format"
    );

    let decoded = CellInputs::from_raw(&encoded);
    assert_eq!(decoded.len(), outputs.len());
    for (i, expected) in outputs.iter().enumerate() {
        assert_eq!(decoded.get(i).raw(), *expected, "slot {i}");
    }
}

/// The single `Interactive` panel kind a widget's output renders as.
fn widget_kind(output: &CellOutput) -> String {
    match output.into_panels().as_slice() {
        [ironpad_cell::DisplayPanel::Interactive { kind, .. }] => kind.clone(),
        other => panic!("expected one Interactive panel, got {other:?}"),
    }
}

#[test]
fn every_cell_widget_kind_is_dispatched() {
    // One of each `ironpad_cell::ui` constructor. A new widget has to be added
    // here, and then fails below until the app renders its kind.
    let emitted: BTreeSet<String> = [
        CellOutput::from(ui::slider("s", 0.0, 1.0)),
        CellOutput::from(ui::dropdown(&["a"])),
        CellOutput::from(ui::checkbox("c")),
        CellOutput::from(ui::text_input("t")),
        CellOutput::from(ui::number(0.0, 1.0)),
        CellOutput::from(ui::switch("w")),
        CellOutput::from(ui::button("b")),
        CellOutput::from(ui::progress_bar()),
    ]
    .iter()
    .map(widget_kind)
    .collect();
    let dispatched: BTreeSet<String> = WIDGET_KINDS.iter().map(|k| (*k).to_owned()).collect();

    for kind in &emitted {
        assert!(
            dispatched.contains(kind),
            "the cell emits widget kind {kind:?}, which the app would render as unknown"
        );
    }
    assert_eq!(
        emitted, dispatched,
        "the app dispatches a widget kind no cell constructor emits"
    );
}

#[test]
fn live_tick_kind_codes_decode_to_the_panel_kind_strings() {
    for content in [
        LiveContent::Text("t".into()),
        LiveContent::Html("<i>h</i>".into()),
        LiveContent::Markdown("# m".into()),
    ] {
        let tick = LiveTickResult::from(content.clone());
        // SAFETY: the content buffer was leaked by `From<LiveContent>` with
        // capacity == len, which is exactly `ironpad_dealloc`'s contract.
        unsafe { ironpad_cell::ironpad_dealloc(tick.content_ptr, tick.content_len) };

        let initial = CellOutput::from(LiveViewMeta {
            fps: 1,
            initial_content: content,
        });
        let panel_kind = match initial.into_panels().as_slice() {
            [ironpad_cell::DisplayPanel::LiveView { kind, .. }] => kind.clone(),
            other => panic!("expected one LiveView panel, got {other:?}"),
        };
        assert_eq!(
            live_kind_str(tick.kind),
            panel_kind,
            "tick code {} decodes differently from the initial panel's kind",
            tick.kind
        );
    }
}
