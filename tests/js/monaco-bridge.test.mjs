/**
 * Monaco bridge tests (public/monaco/bridge.js).
 *
 *   cargo make test-js
 *
 * The bridge hands the Rust side an editor id before Monaco's AMD bundle has
 * loaded, so every call has two lives: queued on the record, then replayed
 * onto the real editor. These run the real bridge against a fake AMD
 * `require` and a fake `monaco`, with the load resolved when a test says so.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BRIDGE_SOURCE = readFileSync(
  path.join(HERE, "..", "..", "public", "monaco", "bridge.js"),
  "utf8",
);

// ── Harness ─────────────────────────────────────────────────────────────────

/**
 * A fake editor: `setValue` fires the content listeners with `isFlush: true`
 * (what Monaco's `setValue` does), and `type` stands in for a keystroke
 * (`isFlush: false`).
 */
function fakeEditor(options) {
  let value = options.value;
  const listeners = [];
  const editor = {
    options,
    actions: [],
    setValueCalls: [],
    disposed: false,
    onDispose: null,
    getValue: () => value,
    setValue(v) {
      editor.setValueCalls.push(v);
      value = v;
      for (const l of listeners) l({ isFlush: true });
    },
    type(v) {
      value = v;
      for (const l of listeners) l({ isFlush: false });
    },
    onDidChangeModelContent(l) {
      listeners.push(l);
    },
    onDidContentSizeChange() {},
    getContentHeight: () => 100,
    layout() {},
    updateOptions() {},
    addAction(action) {
      editor.actions.push(action);
    },
    getModel: () => ({}),
    focus() {},
    dispose() {
      if (editor.onDispose) editor.onDispose();
      editor.disposed = true;
      value = "";
    },
  };
  return editor;
}

function fakeMonaco() {
  const monaco = {
    created: [],
    themes: [],
    languages: {
      registerHoverProvider() {},
      registerCompletionItemProvider() {},
      CompletionItemKind: {},
      CompletionItemInsertTextRule: {},
    },
    editor: {
      create(container, options) {
        const editor = fakeEditor(options);
        monaco.created.push(editor);
        return editor;
      },
      setTheme(name) {
        monaco.themes.push(name);
      },
      setModelMarkers() {},
    },
  };
  return monaco;
}

/**
 * A fresh bridge. Monaco loads only when the test calls `load()`, and
 * `requires` counts how often the bridge asked the AMD loader for it.
 */
function loadBridge() {
  const window = {};
  const monaco = fakeMonaco();
  const pending = [];
  const env = { requires: 0 };
  const require = (deps, cb) => {
    env.requires += 1;
    pending.push(cb);
  };
  const fetch = () => Promise.resolve({ ok: false });
  // eslint-disable-next-line no-new-func
  new Function("window", "require", "fetch", BRIDGE_SOURCE)(window, require, fetch);
  env.bridge = window.IronpadMonaco;
  env.monaco = monaco;
  /** Resolve the AMD load and let the bridge's `.then` callbacks run. */
  env.load = async () => {
    for (const cb of pending.splice(0)) cb(monaco);
    await new Promise((r) => setTimeout(r, 0));
  };
  return env;
}

const container = () => ({ style: {} });

// ── Tests ───────────────────────────────────────────────────────────────────

test("addAction forwards a precondition, queued or live", async () => {
  const { bridge, monaco, load } = loadBridge();
  const id = bridge.create(container(), "x", "markdown", null);
  bridge.addAction(id, "queued.when", [9], () => {}, "!findWidgetVisible");
  bridge.addAction(id, "queued.always", [9], () => {});
  await load();
  bridge.addAction(id, "live.when", [9], () => {}, "!findWidgetVisible");

  const byId = Object.fromEntries(monaco.created[0].actions.map((a) => [a.id, a]));
  assert.equal(byId["queued.when"].precondition, "!findWidgetVisible");
  assert.equal(byId["live.when"].precondition, "!findWidgetVisible");
  assert.equal("precondition" in byId["queued.always"], false);
});

test("dispose unregisters the editor before disposing it", async () => {
  // Disposing Monaco removes its focused widget input, which fires a focusout
  // into Rust while `dispose` is still running. A read there must find no
  // editor, not the half-disposed one (whose value is already gone).
  const { bridge, monaco, load } = loadBridge();
  const id = bridge.create(container(), "# kept text", "markdown", null);
  await load();
  let readDuringDispose = null;
  monaco.created[0].onDispose = () => {
    monaco.created[0].getValue = () => "HALF-DISPOSED";
    readDuringDispose = bridge.getValue(id);
  };
  bridge.dispose(id);
  assert.equal(monaco.created[0].disposed, true);
  assert.equal(readDuringDispose, "");
});

test("getValue returns the initial value before Monaco has loaded", async () => {
  const { bridge, monaco, load } = loadBridge();
  const id = bridge.create(container(), "# original", "markdown", null);
  assert.equal(bridge.getValue(id), "# original");

  await load();
  // The seed is the editor's own value: no redundant setValue replay, and
  // nothing left queued to override a later edit.
  assert.deepEqual(monaco.created[0].setValueCalls, []);
  monaco.created[0].type("# edited");
  assert.equal(bridge.getValue(id), "# edited");
});

test("a value set before Monaco has loaded still wins", async () => {
  const { bridge, monaco, load } = loadBridge();
  const id = bridge.create(container(), "# original", "markdown", null);
  bridge.setValue(id, "# queued");
  assert.equal(bridge.getValue(id), "# queued");
  await load();
  assert.equal(monaco.created[0].getValue(), "# queued");
});

test("a program write does not fire onChange; typing does", async () => {
  // setValue is how the host pulls an agent's edit into Monaco. Echoing it
  // through onChange made the pane look like unsaved typing, which armed a
  // save that reverted the agent's next edit.
  const { bridge, monaco, load } = loadBridge();
  const seen = [];
  const id = bridge.create(container(), "a", "rust", (v) => seen.push(v));
  await load();
  bridge.setValue(id, "agent");
  assert.deepEqual(seen, []);
  monaco.created[0].type("human");
  assert.deepEqual(seen, ["human"]);
});
