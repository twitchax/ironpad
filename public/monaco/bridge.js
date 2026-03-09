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
      function finalize(m) {
        window.monaco = m;
        // Register custom languages and theme (from languages.js).
        if (window.IronpadLanguages && window.IronpadLanguages.register) {
          window.IronpadLanguages.register(m);
        }
        resolve(m);
      }

      if (window.monaco) {
        finalize(window.monaco);
      } else {
        require(["vs/editor/editor.main"], function (m) {
          finalize(m);
        });
      }
    });

    return monacoPromise;
  }

  // Per-editor cell context for the completion provider.
  // Keys are numeric editor IDs, values are { variables: [{name, type, doc}] }.
  var cellContexts = {};

  // Whether the Rust completion provider has been registered.
  var completionProviderRegistered = false;

  function registerCompletionProvider(monaco) {
    if (completionProviderRegistered) return;
    completionProviderRegistered = true;

    var rustKeywords = [
      "let", "fn", "struct", "impl", "enum", "match", "if", "for", "while",
      "loop", "return", "async", "await", "pub", "use", "mod", "trait",
      "where", "type", "const", "static", "mut", "ref", "move", "self",
      "super", "crate"
    ];

    var rustSnippets = [
      { label: "let", insert: "let ${1:name} = ${2:value};", doc: "Variable binding" },
      { label: "letmut", insert: "let mut ${1:name} = ${2:value};", doc: "Mutable variable binding" },
      { label: "fn", insert: "fn ${1:name}(${2}) {\n\t$0\n}", doc: "Function definition" },
      { label: "struct", insert: "struct ${1:Name} {\n\t${2:field}: ${3:Type},\n}", doc: "Struct definition" },
      { label: "impl", insert: "impl ${1:Type} {\n\t$0\n}", doc: "Impl block" },
      { label: "enum", insert: "enum ${1:Name} {\n\t${2:Variant},\n}", doc: "Enum definition" },
      { label: "match", insert: "match ${1:expr} {\n\t${2:pattern} => ${3:expr},\n}", doc: "Match expression" },
      { label: "for", insert: "for ${1:item} in ${2:iter} {\n\t$0\n}", doc: "For loop" },
      { label: "while", insert: "while ${1:cond} {\n\t$0\n}", doc: "While loop" },
      { label: "if", insert: "if ${1:cond} {\n\t$0\n}", doc: "If expression" },
      { label: "iflet", insert: "if let ${1:pattern} = ${2:expr} {\n\t$0\n}", doc: "If-let expression" },
    ];

    var ironpadHelpers = [
      { label: "CellOutput::text", insert: 'CellOutput::text("${1:text}")', doc: "Create text cell output" },
      { label: "CellOutput::html", insert: 'CellOutput::html("${1:html}")', doc: "Create HTML cell output" },
      { label: "CellOutput::svg", insert: 'CellOutput::svg("${1:svg}")', doc: "Create SVG cell output" },
      { label: "Svg", insert: 'Svg("${1:svg}")', doc: "SVG shorthand" },
      { label: "Html", insert: 'Html("${1:html}")', doc: "HTML shorthand" },
    ];

    var commonMacros = [
      { label: "println!", insert: 'println!("${1:{}}", ${2:expr})', doc: "Print to stdout with newline" },
      { label: "format!", insert: 'format!("${1:{}}", ${2:expr})', doc: "Create formatted String" },
      { label: "vec!", insert: "vec![${1:items}]", doc: "Create a Vec" },
      { label: "todo!", insert: 'todo!("${1:reason}")', doc: "Placeholder for unfinished code" },
      { label: "unimplemented!", insert: 'unimplemented!("${1:reason}")', doc: "Mark unimplemented code" },
      { label: "assert!", insert: "assert!(${1:expr})", doc: "Assert condition is true" },
      { label: "assert_eq!", insert: "assert_eq!(${1:left}, ${2:right})", doc: "Assert two values are equal" },
    ];

    // Look up the editor ID for a given Monaco model.
    function editorIdForModel(model) {
      for (var key in editors) {
        var record = editors[key];
        if (record.editor && record.editor.getModel() === model) {
          return Number(key);
        }
      }
      return null;
    }

    monaco.languages.registerCompletionItemProvider("rust", {
      triggerCharacters: [".", ":"],
      provideCompletionItems: function (model, position) {
        var word = model.getWordUntilPosition(position);
        var range = {
          startLineNumber: position.lineNumber,
          startColumn: word.startColumn,
          endLineNumber: position.lineNumber,
          endColumn: word.endColumn,
        };

        var suggestions = [];
        var Kind = monaco.languages.CompletionItemKind;
        var InsertRule = monaco.languages.CompletionItemInsertTextRule;

        // Cell variables from context.
        var editorId = editorIdForModel(model);
        if (editorId !== null && cellContexts[editorId]) {
          var vars = cellContexts[editorId].variables || [];
          for (var i = 0; i < vars.length; i++) {
            var v = vars[i];
            suggestions.push({
              label: v.name,
              kind: Kind.Variable,
              insertText: v.name,
              detail: v.type || "",
              documentation: v.doc || "",
              range: range,
              sortText: "0_" + v.name,
            });
          }
        }

        // Rust keywords.
        for (var k = 0; k < rustKeywords.length; k++) {
          suggestions.push({
            label: rustKeywords[k],
            kind: Kind.Keyword,
            insertText: rustKeywords[k],
            range: range,
            sortText: "2_" + rustKeywords[k],
          });
        }

        // Rust snippets.
        for (var s = 0; s < rustSnippets.length; s++) {
          var sn = rustSnippets[s];
          suggestions.push({
            label: sn.label,
            kind: Kind.Snippet,
            insertText: sn.insert,
            insertTextRules: InsertRule.InsertAsSnippet,
            documentation: sn.doc,
            range: range,
            sortText: "3_" + sn.label,
          });
        }

        // ironpad helpers.
        for (var h = 0; h < ironpadHelpers.length; h++) {
          var ih = ironpadHelpers[h];
          suggestions.push({
            label: ih.label,
            kind: Kind.Function,
            insertText: ih.insert,
            insertTextRules: InsertRule.InsertAsSnippet,
            documentation: ih.doc,
            range: range,
            sortText: "1_" + ih.label,
          });
        }

        // Common macros.
        for (var m = 0; m < commonMacros.length; m++) {
          var cm = commonMacros[m];
          suggestions.push({
            label: cm.label,
            kind: Kind.Function,
            insertText: cm.insert,
            insertTextRules: InsertRule.InsertAsSnippet,
            documentation: cm.doc,
            range: range,
            sortText: "1_" + cm.label,
          });
        }

        return { suggestions: suggestions };
      },
    });
  }

  window.IronpadMonaco = {
    /// Store per-editor cell context for the completion provider.
    /// `context` is an object with `variables: [{name, type, doc}]`.
    setCellContext: function (editorId, context) {
      cellContexts[editorId] = context;
    },

    /// Create a Monaco editor inside `container`.
    /// Returns a numeric editor ID.  The editor is created asynchronously
    /// (Monaco AMD load), but the ID is valid immediately — calls to
    /// getValue / setValue before the editor is ready are safely queued.
    create: function (container, value, language, onChange) {
      var id = nextId++;
      var record = { editor: null, pendingValue: null };
      editors[id] = record;

      ensureMonaco().then(function (monaco) {
        registerCompletionProvider(monaco);

        var editor = monaco.editor.create(container, {
          value: value,
          language: language,
          theme: "ironpad-dark",
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

        // Apply any markers that were set before the editor was ready.
        if (record.pendingMarkers) {
          var model = editor.getModel();
          if (model) {
            window.monaco.editor.setModelMarkers(model, "ironpad", record.pendingMarkers);
          }
          record.pendingMarkers = null;
        }

        // Apply read-only mode if queued before the editor was ready.
        if (record.pendingReadOnly !== undefined) {
          editor.updateOptions({ readOnly: record.pendingReadOnly });
          delete record.pendingReadOnly;
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

    /// Set model markers (inline error/warning decorations) on the editor.
    /// `markers` is an array of objects with:
    ///   { startLineNumber, startColumn, endLineNumber, endColumn, message, severity }
    /// Severity values: 1 = Hint, 2 = Info, 4 = Warning, 8 = Error
    setMarkers: function (id, markers) {
      var record = editors[id];
      if (!record) return;
      if (record.editor) {
        var model = record.editor.getModel();
        if (model) {
          window.monaco.editor.setModelMarkers(model, "ironpad", markers);
        }
      } else {
        // Queue markers until editor is ready.
        record.pendingMarkers = markers;
      }
    },

    /// Clear all ironpad markers from the editor.
    clearMarkers: function (id) {
      var record = editors[id];
      if (!record) return;
      if (record.editor) {
        var model = record.editor.getModel();
        if (model) {
          window.monaco.editor.setModelMarkers(model, "ironpad", []);
        }
      }
      // Also clear any pending markers.
      record.pendingMarkers = null;
    },

    setReadOnly: function (id, readOnly) {
      var record = editors[id];
      if (!record) return;
      if (record.editor) {
        record.editor.updateOptions({ readOnly: readOnly });
      } else {
        record.pendingReadOnly = readOnly;
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
