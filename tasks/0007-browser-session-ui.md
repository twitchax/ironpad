# 0007: Browser Session UI

## Summary

Add UI elements for the user to start/stop agent sessions, view the token, and see agent activity in real-time.

## Motivation

The user needs a way to vend tokens to agents and see what agents are doing in their notebook. This should be low-friction (one click to start) and transparent (agent edits are visually distinct).

## Design

### Start session

- **Location**: Toolbar (alongside Share, Export, etc.) or a dedicated button in the header
- **Interaction**: Click "Start Agent Session" → modal/popover appears with:
  - Generated token (large monospace text)
  - Copy-to-clipboard button
  - One-liner for the agent: `ironpad connect --host https://ironpad.twitchax.com --token <token>`
  - Permission toggles (read/write/execute, defaulting to read+write)
  - "Done" button to dismiss

### Active session indicator

When a session is active:
- Small indicator in the toolbar/status bar (e.g., a dot or icon)
- Shows number of connected agents: "1 agent connected"
- Click to expand session panel

### Session panel

- List of connected agents (by client_id)
- "End Session" button → confirms, then closes all agent connections
- Token (hidden by default, click to reveal)
- Session duration

### Agent activity in cells

When an agent modifies a cell:
- Brief visual indicator on the cell (e.g., a colored border flash, or a small "agent edited" badge that fades after a few seconds)
- The `by` field from the `EventEnvelope` tells the UI who made the change
- If the user is currently editing that cell in Monaco, the content updates live (Monaco's `setValue` with cursor preservation)

### Cell-level conflict UX

If the user is typing in a cell and an agent update arrives for the same cell:
- The model will accept the agent's update (it has the current version)
- The user's next save attempt will get a `VersionConflict`
- UI behavior: show a non-intrusive notification "Agent updated this cell" with option to reload the cell content from the model
- Don't auto-replace what the user is typing — that's jarring

### End session

- "End Session" button in the session panel
- Sends `ControlMessage::EndSession` via WebSocket
- Server disconnects all guests
- WebSocket closes
- UI returns to normal (no session indicator)

### Closing the tab

- `beforeunload` event handler: if session is active, warn the user that agents will be disconnected
- WebSocket closes naturally → server cleans up

## Changes

- **`crates/ironpad-app/src/components/session_panel.rs`** (new): Session UI component
- **`crates/ironpad-app/src/pages/notebook_editor/mod.rs`**: Add session panel to toolbar
- **`crates/ironpad-app/src/pages/notebook_editor/cell_item.rs`**: Add agent activity indicator
- **`style/main.scss`**: Styles for session UI, agent activity indicators

## Dependencies

- **0004** (session/token model — types for display)
- **0006** (browser WebSocket — session lifecycle control)

## Acceptance Criteria

- User can start a session with one click
- Token is displayed and copyable
- Connected agents are listed
- User can end the session
- Agent edits show a brief visual indicator on affected cells
- Tab close warns if session is active
- Session indicator visible in toolbar when active
