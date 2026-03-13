# 0010: Integration Testing

## Summary

End-to-end tests that verify the full flow: browser starts session, CLI connects, edits flow bidirectionally, conflicts are handled, and session lifecycle works correctly.

## Motivation

This feature spans the entire stack (browser, server, CLI). Unit tests on individual layers aren't sufficient — we need to verify that messages actually flow end-to-end and the user experience works.

## Design

### Test infrastructure

- **Playwright** (already set up) for browser-side testing
- **CLI binary** spawned as a subprocess in tests
- **Test server** started per test suite (existing pattern from `tests/e2e/`)

### Test scenarios

#### 1. Session lifecycle
```
1. Open notebook in browser (Playwright)
2. Click "Start Agent Session"
3. Verify token is displayed
4. Spawn CLI: `ironpad-cli connect --token <token>`
5. Verify CLI reports connected
6. Verify browser shows "1 agent connected"
7. Click "End Session"
8. Verify CLI exits with session-ended error
9. Verify browser returns to normal state
```

#### 2. Agent reads notebook
```
1. Browser has notebook with 3 cells (pre-populated)
2. Start session, connect CLI
3. CLI: `cells list` → verify 3 cells with correct order
4. CLI: `cells get <id>` → verify source matches browser
```

#### 3. Agent edits cell
```
1. Browser has notebook with a cell containing "let x = 1;"
2. Start session, connect CLI
3. CLI: `cells update <id> --source "let x = 42;"`
4. Verify browser's Monaco editor shows "let x = 42;"
5. Verify cell has agent activity indicator
```

#### 4. User edits cell, agent sees it
```
1. Start session, connect CLI
2. CLI: `watch` (background, captures events)
3. User types in Monaco editor in browser
4. Verify CLI receives CellUpdated event with new source
```

#### 5. Concurrent edit (OCC conflict)
```
1. Cell at version 3
2. CLI reads cell (gets version 3)
3. User edits cell in browser (now version 4)
4. CLI sends update with version 3
5. Verify CLI gets VersionConflict error with actual version 4
6. CLI re-reads cell, retries with version 4
7. Verify update succeeds
```

#### 6. Agent adds cell
```
1. Browser has 2 cells
2. CLI: `cells add --source "println!(\"from agent\");" --after <cell_1_id>`
3. Verify browser shows 3 cells in correct order
4. Verify new cell appears between cell 1 and cell 2
```

#### 7. Agent deletes cell
```
1. Browser has 3 cells
2. CLI: `cells delete <cell_2_id>`
3. Verify browser shows 2 cells
4. Verify remaining cells have correct order
```

#### 8. Session ends on tab close
```
1. Start session, connect CLI
2. Close browser tab (Playwright page.close())
3. Verify CLI daemon detects disconnect
4. Verify CLI commands fail with session-ended error
```

#### 9. Permission enforcement
```
1. Start session with default permissions (read + write, no execute)
2. CLI: `cells list` → succeeds
3. CLI: `cells update ...` → succeeds
4. CLI: `compile <id>` → fails with permission denied
```

#### 10. Multiple agents
```
1. Start session, get token
2. Connect CLI agent 1 with token
3. Connect CLI agent 2 with same token
4. Agent 1 edits cell → agent 2 receives event
5. Verify browser shows 2 agents connected
```

### Test structure

```
tests/
  e2e/
    session.spec.ts          # scenarios 1, 8
    agent-read.spec.ts       # scenario 2
    agent-write.spec.ts      # scenarios 3, 6, 7
    bidirectional.spec.ts    # scenario 4
    conflict.spec.ts         # scenario 5
    permissions.spec.ts      # scenario 9
    multi-agent.spec.ts      # scenario 10
```

### Test helpers

- `startSession(page)` — click button, extract token
- `connectCli(token, host)` — spawn CLI subprocess, return handle
- `cliExec(handle, command)` — send command, parse JSON response
- `waitForEvent(handle, eventType)` — wait for event on CLI watch stream

## Changes

- **`tests/e2e/session.spec.ts`** (new): Session lifecycle tests
- **`tests/e2e/agent-*.spec.ts`** (new): Agent interaction tests
- **`tests/e2e/helpers/cli.ts`** (new): CLI subprocess helpers
- **`tests/e2e/helpers/session.ts`** (new): Session management helpers

## Dependencies

- **All previous tasks** (0001–0009)

## Acceptance Criteria

- All 10 test scenarios pass
- Tests run in CI (`cargo make uat`)
- Tests are hermetic (each test gets its own server instance + notebook)
- Test execution time < 2 minutes for the full suite
