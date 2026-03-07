// Monaco editor bridge for ironpad.
// Provides a simple global API (window.IronpadMonaco) that the Rust/WASM side
// can call via wasm-bindgen to create, read, write, and dispose Monaco editors.

(function () {
  "use strict";

  var editors = {};
  var nextId = 1;
  var monacoPromise = null;

  function ensureMonaco() {
    if (monacoPromise) return monacoPromise;

    monacoPromise = new Promise(function (resolve) {
      if (window.monaco) {
        resolve(window.monaco);
      } else {
        require(["vs/editor/editor.main"], function (m) {
          window.monaco = m;
          resolve(m);
        });
      }
    });

    return monacoPromise;
  }

  window.IronpadMonaco = {
    /// Create a Monaco editor inside `container`.
    /// Returns a numeric editor ID.  The editor is created asynchronously
    /// (Monaco AMD load), but the ID is valid immediately — calls to
    /// getValue / setValue before the editor is ready are safely queued.
    create: function (container, value, language, onChange) {
      var id = nextId++;
      var record = { editor: null, pendingValue: null };
      editors[id] = record;

      ensureMonaco().then(function (monaco) {
        var editor = monaco.editor.create(container, {
          value: value,
          language: language,
          theme: "vs-dark",
          minimap: { enabled: false },
          lineNumbers: "on",
          automaticLayout: true,
          wordWrap: "on",
          fontSize: 14,
          scrollBeyondLastLine: false,
          renderLineHighlight: "all",
          padding: { top: 8, bottom: 8 },
        });

        record.editor = editor;

        // Apply any value that was set before the editor was ready.
        if (record.pendingValue !== null) {
          editor.setValue(record.pendingValue);
          record.pendingValue = null;
        }

        if (onChange) {
          editor.onDidChangeModelContent(function () {
            onChange(editor.getValue());
          });
        }
      });

      return id;
    },

    getValue: function (id) {
      var record = editors[id];
      if (!record) return "";
      if (record.editor) return record.editor.getValue();
      return record.pendingValue !== null ? record.pendingValue : "";
    },

    setValue: function (id, value) {
      var record = editors[id];
      if (!record) return;
      if (record.editor) {
        record.editor.setValue(value);
      } else {
        record.pendingValue = value;
      }
    },

    dispose: function (id) {
      var record = editors[id];
      if (!record) return;
      if (record.editor) {
        record.editor.dispose();
      }
      delete editors[id];
    },
  };
})();
