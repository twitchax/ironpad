// Monaco language configuration for ironpad.
// Registers a proper TOML monarch grammar and defines the custom "ironpad-dark" theme.

(function () {
  "use strict";

  // ── TOML Monarch Language Definition ──────────────────────────────────────

  var tomlLanguage = {
    defaultToken: "",
    tokenPostfix: ".toml",

    // Brackets for auto-closing and matching.
    brackets: [
      { open: "[", close: "]", token: "delimiter.square" },
      { open: "{", close: "}", token: "delimiter.curly" },
    ],

    keywords: ["true", "false"],

    tokenizer: {
      root: [
        // Whitespace.
        { include: "@whitespace" },

        // Table headers: [section] and [[array-of-tables]].
        [/\[\[/, "metatag", "@doubleTableKey"],
        [/\[/, "metatag", "@tableKey"],

        // Key-value pairs.
        { include: "@keyValue" },
      ],

      whitespace: [
        [/[ \t\r\n]+/, ""],
        [/#.*$/, "comment"],
      ],

      tableKey: [
        [/[^\]\s.]+/, "metatag"],
        [/\./, "delimiter"],
        [/\]/, "metatag", "@pop"],
      ],

      doubleTableKey: [
        [/[^\]\s.]+/, "metatag"],
        [/\./, "delimiter"],
        [/\]\]/, "metatag", "@pop"],
      ],

      keyValue: [
        // Bare key or dotted key.
        [/[A-Za-z0-9_-]+/, { cases: { "@keywords": "keyword", "@default": "key" } }],
        // Quoted key.
        [/"/, "key", "@quotedKey"],
        [/'/, "key", "@literalKey"],
        // Equals sign.
        [/=/, "delimiter"],
        // Values.
        { include: "@value" },
      ],

      quotedKey: [
        [/[^\\"]+/, "key"],
        [/\\./, "key.escape"],
        [/"/, "key", "@pop"],
      ],

      literalKey: [
        [/[^']+/, "key"],
        [/'/, "key", "@pop"],
      ],

      value: [
        // Datetime (must come before numbers to avoid partial matches).
        [/\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?/, "number.date"],
        [/\d{4}-\d{2}-\d{2}/, "number.date"],
        [/\d{2}:\d{2}:\d{2}(?:\.\d+)?/, "number.date"],

        // Numbers: hex, octal, binary, float, integer.
        [/0x[0-9a-fA-F_]+/, "number.hex"],
        [/0o[0-7_]+/, "number.octal"],
        [/0b[01_]+/, "number.binary"],
        [/[+-]?(?:inf|nan)/, "number.float"],
        [/[+-]?\d[\d_]*\.\d[\d_]*(?:[eE][+-]?\d[\d_]*)?/, "number.float"],
        [/[+-]?\d[\d_]*[eE][+-]?\d[\d_]*/, "number.float"],
        [/[+-]?\d[\d_]*/, "number"],

        // Booleans.
        [/\b(?:true|false)\b/, "keyword"],

        // Multi-line basic strings (triple-quote).
        [/"""/, "string", "@mlBasicString"],
        // Basic strings.
        [/"/, "string", "@basicString"],
        // Multi-line literal strings (triple single-quote).
        [/'''/, "string", "@mlLiteralString"],
        // Literal strings.
        [/'/, "string", "@literalString"],

        // Inline table.
        [/\{/, "delimiter.curly", "@inlineTable"],

        // Array.
        [/\[/, "delimiter.square", "@array"],

        // Comma (in inline tables / arrays).
        [/,/, "delimiter"],
      ],

      basicString: [
        [/[^\\"]+/, "string"],
        [/\\[btnfr"\\]/, "string.escape"],
        [/\\u[0-9a-fA-F]{4}/, "string.escape"],
        [/\\U[0-9a-fA-F]{8}/, "string.escape"],
        [/"/, "string", "@pop"],
      ],

      mlBasicString: [
        [/[^\\"]+/, "string"],
        [/\\[btnfr"\\]/, "string.escape"],
        [/\\u[0-9a-fA-F]{4}/, "string.escape"],
        [/\\U[0-9a-fA-F]{8}/, "string.escape"],
        [/\\\n/, "string.escape"],
        [/"""/, "string", "@pop"],
        [/"/, "string"],
      ],

      literalString: [
        [/[^']+/, "string"],
        [/'/, "string", "@pop"],
      ],

      mlLiteralString: [
        [/[^']+/, "string"],
        [/'''/, "string", "@pop"],
        [/'/, "string"],
      ],

      inlineTable: [
        { include: "@whitespace" },
        { include: "@keyValue" },
        [/,/, "delimiter"],
        [/\}/, "delimiter.curly", "@pop"],
      ],

      array: [
        { include: "@whitespace" },
        { include: "@value" },
        [/,/, "delimiter"],
        [/\]/, "delimiter.square", "@pop"],
      ],
    },
  };

  var tomlLanguageConfiguration = {
    comments: {
      lineComment: "#",
    },
    brackets: [
      ["{", "}"],
      ["[", "]"],
    ],
    autoClosingPairs: [
      { open: "{", close: "}" },
      { open: "[", close: "]" },
      { open: '"', close: '"' },
      { open: "'", close: "'" },
      { open: '"""', close: '"""' },
      { open: "'''", close: "'''" },
    ],
    surroundingPairs: [
      { open: "{", close: "}" },
      { open: "[", close: "]" },
      { open: '"', close: '"' },
      { open: "'", close: "'" },
    ],
  };

  // ── Custom "ironpad-dark" Theme ───────────────────────────────────────────
  //
  // Based on vs-dark, customised to match ironpad's UI palette:
  //   bg: #1a1a2e, header/card: #16213e, border: #0f3460,
  //   text: #eaeaea, muted: #8888aa, accent: #e94560,
  //   success: #40c080, warning: #f0c040, info: #40a0f0.

  var ironpadDarkTheme = {
    base: "vs-dark",
    inherit: true,
    rules: [
      // Comments.
      { token: "comment", foreground: "6a6a8a", fontStyle: "italic" },

      // Strings.
      { token: "string", foreground: "a8d8a0" },
      { token: "string.escape", foreground: "d4a8e0" },

      // Numbers & dates.
      { token: "number", foreground: "d4a8e0" },
      { token: "number.float", foreground: "d4a8e0" },
      { token: "number.hex", foreground: "d4a8e0" },
      { token: "number.octal", foreground: "d4a8e0" },
      { token: "number.binary", foreground: "d4a8e0" },
      { token: "number.date", foreground: "d4a8e0" },

      // Keywords (true/false, control flow).
      { token: "keyword", foreground: "e94560", fontStyle: "bold" },
      { token: "keyword.control", foreground: "e94560" },

      // Types.
      { token: "type", foreground: "40c0c0" },
      { token: "type.identifier", foreground: "40c0c0" },

      // Functions / identifiers.
      { token: "entity.name.function", foreground: "60b0f0" },
      { token: "support.function", foreground: "60b0f0" },

      // Operators / delimiters.
      { token: "delimiter", foreground: "b0b0cc" },
      { token: "delimiter.square", foreground: "b0b0cc" },
      { token: "delimiter.curly", foreground: "b0b0cc" },
      { token: "operator", foreground: "b0b0cc" },

      // TOML-specific tokens.
      { token: "metatag", foreground: "e9a060" },
      { token: "key", foreground: "60b0f0" },

      // Rust-specific tokens (from Monaco's built-in Rust grammar).
      { token: "keyword.rust", foreground: "e94560" },
      { token: "attribute.rust", foreground: "e9a060" },
      { token: "string.quoted.double.rust", foreground: "a8d8a0" },
      { token: "lifetime.rust", foreground: "d4a8e0", fontStyle: "italic" },
    ],
    colors: {
      // Editor chrome.
      "editor.background": "#1a1a2e",
      "editor.foreground": "#eaeaea",
      "editor.lineHighlightBackground": "#16213e",
      "editor.selectionBackground": "#0f346080",
      "editor.inactiveSelectionBackground": "#0f346040",

      // Cursor.
      "editorCursor.foreground": "#e94560",

      // Line numbers.
      "editorLineNumber.foreground": "#4a4a6a",
      "editorLineNumber.activeForeground": "#8888aa",

      // Indentation guides.
      "editorIndentGuide.background": "#2a2a4a",
      "editorIndentGuide.activeBackground": "#3a3a5a",

      // Bracket matching.
      "editorBracketMatch.background": "#e9456020",
      "editorBracketMatch.border": "#e94560",

      // Widget (autocomplete, hover) styling.
      "editorWidget.background": "#16213e",
      "editorWidget.border": "#0f3460",
      "editorSuggestWidget.background": "#16213e",
      "editorSuggestWidget.border": "#0f3460",
      "editorSuggestWidget.selectedBackground": "#0f3460",

      // Gutter / margins.
      "editorGutter.background": "#1a1a2e",

      // Overview ruler (scrollbar decorations).
      "editorOverviewRuler.border": "#0f3460",

      // Scrollbar.
      "scrollbarSlider.background": "#3a3a5a80",
      "scrollbarSlider.hoverBackground": "#4a4a6aa0",
      "scrollbarSlider.activeBackground": "#5a5a7ac0",

      // Minimap (disabled, but just in case).
      "minimap.background": "#1a1a2e",

      // Find / search highlights.
      "editor.findMatchBackground": "#e9456040",
      "editor.findMatchHighlightBackground": "#e9456020",

      // Error / warning squiggles.
      "editorError.foreground": "#e94560",
      "editorWarning.foreground": "#f0c040",
      "editorInfo.foreground": "#40a0f0",

      // Peek view.
      "peekView.border": "#0f3460",
      "peekViewEditor.background": "#16213e",
      "peekViewResult.background": "#1a1a2e",
    },
  };

  // ── Custom "ironpad-light" Theme ──────────────────────────────────────────
  //
  // Based on vs, customised to match ironpad's light-mode UI palette:
  //   bg: #f5f6fa, surface: #ffffff, border: #d0d5e0,
  //   text: #1a1a2e, muted: #8888aa, accent: #d63851.

  var ironpadLightTheme = {
    base: "vs",
    inherit: true,
    rules: [
      { token: "comment", foreground: "8888aa", fontStyle: "italic" },
      { token: "string", foreground: "2a7040" },
      { token: "string.escape", foreground: "8050d0" },
      { token: "number", foreground: "8050d0" },
      { token: "number.float", foreground: "8050d0" },
      { token: "keyword", foreground: "d63851", fontStyle: "bold" },
      { token: "keyword.control", foreground: "d63851" },
      { token: "type", foreground: "2080a0" },
      { token: "type.identifier", foreground: "2080a0" },
      { token: "entity.name.function", foreground: "3060c0" },
      { token: "support.function", foreground: "3060c0" },
      { token: "delimiter", foreground: "444466" },
      { token: "operator", foreground: "444466" },
      { token: "metatag", foreground: "c07030" },
      { token: "key", foreground: "3060c0" },
      { token: "keyword.rust", foreground: "d63851" },
      { token: "attribute.rust", foreground: "c07030" },
      { token: "string.quoted.double.rust", foreground: "2a7040" },
      { token: "lifetime.rust", foreground: "8050d0", fontStyle: "italic" },
    ],
    colors: {
      "editor.background": "#f5f6fa",
      "editor.foreground": "#1a1a2e",
      "editor.lineHighlightBackground": "#eef0f5",
      "editor.selectionBackground": "#d0d5e080",
      "editor.inactiveSelectionBackground": "#d0d5e040",
      "editorCursor.foreground": "#d63851",
      "editorLineNumber.foreground": "#b0b8c8",
      "editorLineNumber.activeForeground": "#666688",
      "editorIndentGuide.background": "#d0d5e0",
      "editorIndentGuide.activeBackground": "#b0b8c8",
      "editorBracketMatch.background": "#d6385120",
      "editorBracketMatch.border": "#d63851",
      "editorWidget.background": "#ffffff",
      "editorWidget.border": "#d0d5e0",
      "editorSuggestWidget.background": "#ffffff",
      "editorSuggestWidget.border": "#d0d5e0",
      "editorSuggestWidget.selectedBackground": "#eef0f5",
      "editorGutter.background": "#f5f6fa",
      "editorOverviewRuler.border": "#d0d5e0",
      "scrollbarSlider.background": "#b0b8c880",
      "scrollbarSlider.hoverBackground": "#9098a8a0",
      "scrollbarSlider.activeBackground": "#8088a0c0",
      "minimap.background": "#f5f6fa",
      "editor.findMatchBackground": "#d6385140",
      "editor.findMatchHighlightBackground": "#d6385120",
      "editorError.foreground": "#d63851",
      "editorWarning.foreground": "#c09030",
      "editorInfo.foreground": "#3080d0",
      "peekView.border": "#d0d5e0",
      "peekViewEditor.background": "#ffffff",
      "peekViewResult.background": "#f5f6fa",
    },
  };

  // ── Registration function ─────────────────────────────────────────────────

  // Called by the Monaco bridge after the editor AMD module is loaded.
  window.IronpadLanguages = {
    register: function (monaco) {
      // Register TOML language.
      monaco.languages.register({ id: "toml" });
      monaco.languages.setMonarchTokensProvider("toml", tomlLanguage);
      monaco.languages.setLanguageConfiguration("toml", tomlLanguageConfiguration);

      // Define the custom ironpad themes.
      monaco.editor.defineTheme("ironpad-dark", ironpadDarkTheme);
      monaco.editor.defineTheme("ironpad-light", ironpadLightTheme);

      // Apply the theme that matches the current document theme.
      var currentTheme = document.documentElement.getAttribute("data-theme");
      monaco.editor.setTheme(currentTheme === "light" ? "ironpad-light" : "ironpad-dark");
    },
  };
})();
