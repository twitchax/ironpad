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

        // Apply any actions that were queued before the editor was ready.
        if (record.pendingActions) {
          record.pendingActions.forEach(function (action) {
            editor.addAction(action);
          });
          record.pendingActions = null;
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

    addAction: function (id, actionId, keybindings, callback) {
      var record = editors[id];
      if (!record) return;

      var action = {
        id: actionId,
        label: actionId,
        keybindings: keybindings,
        run: function () {
          callback();
        },
      };

      if (record.editor) {
        record.editor.addAction(action);
      } else {
        // Queue action registration until the editor is ready.
        if (!record.pendingActions) record.pendingActions = [];
        record.pendingActions.push(action);
      }
    },

    focus: function (id) {
      var record = editors[id];
      if (!record) return;
      if (record.editor) {
        record.editor.focus();
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
