import { test, expect, type Page } from "@playwright/test";
import {
  ADD_CODE,
  ADD_MARKDOWN,
  createNotebook,
  startSession,
  endSession,
  waitForPersistedCells,
} from "./helpers/session";
import {
  connectCli,
  cliExec,
  cliExecRaw,
  cliExecRawAsync,
  stopCli,
  CliHandle,
} from "./helpers/cli";
import { gateCompiles } from "./helpers/compile-gate";
import { trackJsErrors } from "./helpers/errors";

/** The first cell's source as IndexedDB holds it. */
function storedSource(page: Page, notebookId: string): Promise<string> {
  return page.evaluate(async (id) => {
    const nb = await (window as any).IronpadStorage.getNotebook(id);
    return nb?.cells?.[0]?.source ?? "";
  }, notebookId);
}

/** Close the session panel so it cannot sit over the cells. */
async function dismissSessionPanel(page: Page): Promise<void> {
  await page.locator(".ironpad-editor").click({ position: { x: 5, y: 5 } });
  await expect(page.locator(".ironpad-session-panel")).toHaveCount(0);
}

// Session tests share a single daemon socket (under a per-run temp HOME; see
// helpers/cli.ts) and must run serially to avoid conflicts.
test.describe.serial("Agent Session", () => {
  let cliHandle: CliHandle | null = null;

  test.afterEach(async () => {
    if (cliHandle) {
      stopCli(cliHandle);
      cliHandle = null;
    }
  });

  test("session lifecycle: start, connect, end", async ({ page }) => {
    test.setTimeout(60_000);

    const jsErrors = trackJsErrors(page);

    // Create a new notebook.
    await createNotebook(page);

    // The session control is an icon-only square in the toolbar's right-hand
    // group, beside menu/gear/close. It carries no visible label BY DESIGN:
    // as text it swung from "Start Agent Session" to "2 agents" on every
    // connect and dragged its neighbours sideways. The wording lives in the
    // tooltip, and the width must not move when a guest joins.
    const sessionBtn = page.locator(
      ".ironpad-toolbar-right .ironpad-session-button"
    );
    await expect(sessionBtn).toBeVisible({ timeout: 15_000 });
    await expect(sessionBtn).toHaveAttribute("title", /Start Agent Session/);
    const widthBefore = (await sessionBtn.boundingBox())!.width;

    // Start session and get token.
    const token = await startSession(page);
    expect(token).toHaveLength(64);

    // Connect CLI daemon.
    cliHandle = await connectCli(token);

    // Verify daemon reports connected.
    const status = cliExec(["status"]);
    expect(status.connected).toBe(true);
    expect(status.cached).toBe(true);

    // Verify browser shows agent connected: the badge carries the count and
    // the tooltip the wording. The button's WIDTH must be unchanged — that
    // invariant is the whole reason this control is an icon.
    await expect(page.locator(".ironpad-session-badge")).toHaveText("1", {
      timeout: 5_000,
    });
    await expect(sessionBtn).toHaveAttribute("title", /1 agent connected/);
    expect((await sessionBtn.boundingBox())!.width).toBe(widthBefore);

    // End session from browser.
    await endSession(page);

    // Verify CLI commands fail after the session ends. Poll rather than sleep:
    // the daemon needs a moment to observe the WS close, and `status` then
    // exits non-zero.
    await expect
      .poll(() => cliExecRaw(["status"]).exitCode, { timeout: 10_000 })
      .not.toBe(0);

    expect(jsErrors).toEqual([]);
  });

  // The panel shows a token and a copy-paste CLI command: something you open,
  // read, and click away from. Dismissal must key off the WRAPPER, not the
  // document, or revealing the token would close the thing showing it.
  test("session panel dismisses on an outside click, not an inside one", async ({
    page,
  }) => {
    test.setTimeout(60_000);

    await createNotebook(page);
    await startSession(page);

    const panel = page.locator(".ironpad-session-panel");
    await expect(panel).toBeVisible();

    // Inside: the Show/Hide toggle re-renders the panel's own content, which
    // is exactly the case a naive `target.closest()` check gets wrong.
    await page.locator(".ironpad-session-token-toggle").click();
    await expect(panel).toBeVisible();

    // Outside: anywhere in the editor body.
    await page.locator(".ironpad-editor").click({ position: { x: 5, y: 5 } });
    await expect(panel).toHaveCount(0);

    // Dismissing is not ending: the session is still live and the button
    // reopens the same panel.
    await expect(page.locator(".ironpad-session-button--active")).toBeVisible();
    await page.locator(".ironpad-session-button").click();
    await expect(panel).toBeVisible();

    await endSession(page);
  });

  test("agent reads notebook cells", async ({ page }) => {
    test.setTimeout(60_000);

    // Create notebook with a cell.
    await createNotebook(page);

    // Add a cell via UI.
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);

    // Wait for the cell to actually land in IndexedDB (persistence is
    // fire-and-forget; a fixed sleep raced it).
    await waitForPersistedCells(page, 1);

    // Start session and connect CLI.
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // List cells.
    const cells = cliExec(["cells", "list"]);
    expect(cells).toHaveLength(1);
    expect(cells[0].label).toBeTruthy();
    expect(cells[0].cell_type).toBe("Code");

    // Get cell details.
    const cell = cliExec(["cells", "get", cells[0].id]);
    expect(cell.source).toBeTruthy();
    expect(cell.id).toBe(cells[0].id);
  });

  test("agent adds and deletes cells", async ({ page }) => {
    test.setTimeout(60_000);

    // Create empty notebook.
    await createNotebook(page);

    // Start session and connect.
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Add a cell via CLI.
    const addResult = cliExecRaw([
      "cells",
      "add",
      "--source",
      "let x = 42;",
      "--label",
      "Agent Cell",
    ]);
    expect(addResult.exitCode).toBe(0);

    // Verify browser shows the new cell.
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1, {
      timeout: 15_000,
    });

    // List and verify.
    const cells = cliExec(["cells", "list"]);
    expect(cells).toHaveLength(1);

    // Delete the cell.
    const deleteResult = cliExecRaw(["cells", "delete", cells[0].id]);
    expect(deleteResult.exitCode).toBe(0);

    // Verify browser shows no cells.
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(0, {
      timeout: 15_000,
    });
  });

  test("agent updates cell source", async ({ page }) => {
    test.setTimeout(60_000);

    // Create notebook with a cell.
    await createNotebook(page);
    await page.locator(".ironpad-add-cell-btn").first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await waitForPersistedCells(page, 1);

    // Start session and connect.
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Get cell ID.
    const cells = cliExec(["cells", "list"]);
    const cellId = cells[0].id;

    // Update source via CLI.
    const updateResult = cliExecRaw([
      "cells",
      "update",
      cellId,
      "--source",
      "let x = 99;",
    ]);
    expect(updateResult.exitCode).toBe(0);

    // Verify the cell source was updated in the daemon cache.
    const cell = cliExec(["cells", "get", cellId]);
    expect(cell.source).toContain("let x = 99;");
  });

  test("agent Cargo.toml edit reaches the host editor and survives a host save", async ({
    page,
  }) => {
    // Regression (review editor-1): the host never refreshed its Cargo.toml
    // pane from an agent edit, so a Run compiled the stale local manifest,
    // and every save flush re-sent that stale copy over the agent's edit.
    test.setTimeout(60_000);

    const jsErrors = trackJsErrors(page);

    await createNotebook(page);
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];
    await page.locator(ADD_CODE).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await waitForPersistedCells(page, 1);

    const token = await startSession(page);
    cliHandle = await connectCli(token);
    const cellId = cliExec(["cells", "list"])[0].id;

    const marker = "agent-cargo-marker";
    const update = cliExecRaw([
      "cells",
      "update",
      cellId,
      "--cargo-toml",
      `[dependencies]\n# ${marker}\n`,
    ]);
    expect(update.exitCode).toBe(0);
    await expect
      .poll(() => cliExec(["cells", "get", cellId]).cargo_toml ?? "", {
        timeout: 10_000,
      })
      .toContain(marker);

    // The host's Cargo.toml pane shows the agent's manifest (the same
    // refresh updates the signal a Run compiles from).
    const cell = page.locator(".ironpad-cell-card").first();
    await cell
      .locator(".ironpad-cell-tab", { hasText: "Cargo.toml" })
      .dispatchEvent("click");
    await expect(
      cell.locator(".ironpad-cell-editor-pane").nth(1).locator(".view-lines")
    ).toContainText(marker, { timeout: 10_000 });

    // A host save keeps it. The label rename after Ctrl+S is an ordering
    // barrier: browser events reach the daemon in order, so once the new
    // label is visible there, anything the save flush sent has landed too.
    await page.keyboard.press("Control+s");
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Notebook saved" })
    ).toBeVisible({ timeout: 5_000 });
    const labelInput = cell.locator(".ironpad-cell-label-input");
    await labelInput.fill("After Save");
    await labelInput.blur();
    await expect
      .poll(() => cliExec(["cells", "get", cellId]).label, { timeout: 10_000 })
      .toBe("After Save");
    expect(cliExec(["cells", "get", cellId]).cargo_toml).toContain(marker);

    // And the durable copy agrees.
    await expect
      .poll(
        () =>
          page.evaluate(async (id) => {
            const nb = await (window as any).IronpadStorage.getNotebook(id);
            return nb?.cells?.[0]?.cargo_toml ?? "";
          }, notebookId),
        { timeout: 10_000 },
      )
      .toContain(marker);

    expect(jsErrors).toEqual([]);
  });

  // Regression (review bug-components-4): the markdown cell took its source
  // once, at mount, so an agent's edit never reached the preview, and the
  // next open-and-close wrote the stale editor text back over it.
  test("agent markdown edits reach the preview and survive an untouched open", async ({
    page,
  }) => {
    test.setTimeout(90_000);

    await createNotebook(page);
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];
    await page.locator(ADD_MARKDOWN).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await waitForPersistedCells(page, 1);

    const token = await startSession(page);
    cliHandle = await connectCli(token);
    await dismissSessionPanel(page);
    const cellId = cliExec(["cells", "list"])[0].id;
    const md = page.locator(".ironpad-cell-card").first();
    const preview = md.locator(".ironpad-markdown-cell-preview");
    const editor = md.locator(".ironpad-markdown-cell-editor");

    expect(
      cliExecRaw(["cells", "update", cellId, "--source", "# AGENT TEXT"]).exitCode,
    ).toBe(0);
    await expect(preview).toContainText("AGENT TEXT", { timeout: 10_000 });

    // Open, click in, click away: nothing typed, so nothing may be written.
    await preview.dblclick();
    await expect(editor.locator(".monaco-editor")).toBeVisible({ timeout: 15_000 });
    await editor.locator(".view-lines").click();
    await page.locator(".ironpad-status-bar").click();
    await expect(editor).toHaveCount(0);
    await page.waitForTimeout(2_000); // past the 1s save debounce
    expect(await storedSource(page, notebookId)).toBe("# AGENT TEXT");
    expect(cliExec(["cells", "get", cellId]).source).toBe("# AGENT TEXT");

    // An edit landing while the editor is open and untouched shows on close.
    await preview.dblclick();
    await expect(editor.locator(".monaco-editor")).toBeVisible({ timeout: 15_000 });
    expect(
      cliExecRaw(["cells", "update", cellId, "--source", "# SECOND AGENT TEXT"])
        .exitCode,
    ).toBe(0);
    await expect
      .poll(() => storedSource(page, notebookId), { timeout: 10_000 })
      .toBe("# SECOND AGENT TEXT");
    await editor.locator(".view-lines").click();
    await page.locator(".ironpad-status-bar").click();
    await expect(editor).toHaveCount(0);
    await expect(preview).toContainText("SECOND AGENT TEXT");
    await page.waitForTimeout(2_000);
    expect(await storedSource(page, notebookId)).toBe("# SECOND AGENT TEXT");
  });

  // Regression (review soc-app-client-1): pulling an agent's edit into Monaco
  // fired the editor's change callback like typing does, so the pane looked
  // dirty and armed a 1s save of what it held. A second agent edit inside
  // that second was skipped as "unsaved host typing" and then reverted.
  test("two rapid agent source edits both land", async ({ page }) => {
    test.setTimeout(90_000);

    await createNotebook(page);
    await page.locator(ADD_CODE).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await waitForPersistedCells(page, 1);

    const token = await startSession(page);
    cliHandle = await connectCli(token);
    await dismissSessionPanel(page);
    const cellId = cliExec(["cells", "list"])[0].id;
    const before = cliExec(["cells", "get", cellId]).version;

    expect(
      cliExecRaw(["cells", "update", cellId, "--source", "let first_edit = 1;\nfirst_edit"])
        .exitCode,
    ).toBe(0);
    expect(
      cliExecRaw(["cells", "update", cellId, "--source", "let second_edit = 2;\nsecond_edit"])
        .exitCode,
    ).toBe(0);
    await page.waitForTimeout(2_000); // past the debounce the echo used to arm

    const cell = cliExec(["cells", "get", cellId]);
    expect(cell.source).toBe("let second_edit = 2;\nsecond_edit");
    expect(cell.version).toBe(before + 2);
    const host = page.locator(".ironpad-cell-card").first();
    await expect(host.locator(".view-lines").first()).toContainText("second_edit");
    await expect(host.locator(".ironpad-tab-dirty")).toHaveCount(0);
  });

  // The echo above was also the only thing that live-checked an agent's edit.
  // Asserted on the dispatch rather than the squiggle: the check itself is a
  // cargo round-trip, which live-check.spec.ts covers.
  test("an agent edit gets a live check", async ({ page }) => {
    test.setTimeout(90_000);

    await createNotebook(page);
    await page.locator(ADD_CODE).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await waitForPersistedCells(page, 1);

    const token = await startSession(page);
    cliHandle = await connectCli(token);
    await dismissSessionPanel(page);
    const cellId = cliExec(["cells", "list"])[0].id;

    const checked = page.waitForRequest(
      (r) =>
        r.url().includes("check_cell") &&
        (r.postData() ?? "").includes("agent_checked"),
      { timeout: 30_000 },
    );
    expect(
      cliExecRaw([
        "cells",
        "update",
        cellId,
        "--source",
        'let agent_checked: i32 = "no";\nagent_checked',
      ]).exitCode,
    ).toBe(0);
    await checked;
  });

  // Regression (review bug-editor-21): the label and collapse defaults were
  // seeded once at mount, so an agent's rename never showed, and blurring the
  // label wrote the stale local value back over it.
  test("agent renames and collapse defaults reach the cell header", async ({
    page,
  }) => {
    test.setTimeout(90_000);

    await createNotebook(page);
    await page.locator(ADD_CODE).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await waitForPersistedCells(page, 1);

    const token = await startSession(page);
    cliHandle = await connectCli(token);
    await dismissSessionPanel(page);
    const cellId = cliExec(["cells", "list"])[0].id;
    const cell = page.locator(".ironpad-cell-card").first();

    expect(
      cliExecRaw(["cells", "update", cellId, "--label", "AGENT LABEL"]).exitCode,
    ).toBe(0);
    const label = cell.locator(".ironpad-cell-label-input");
    await expect(label).toHaveValue("AGENT LABEL", { timeout: 10_000 });

    // Focus and blur without typing: nothing may be written back.
    const version = cliExec(["cells", "get", cellId]).version;
    await label.focus();
    await label.blur();
    await page.waitForTimeout(1_000);
    const after = cliExec(["cells", "get", cellId]);
    expect(after.label).toBe("AGENT LABEL");
    expect(after.version).toBe(version);

    // The collapse default follows the agent too, so the toggle flips it back.
    expect(
      cliExecRaw(["cells", "update", cellId, "--collapsed", "true"]).exitCode,
    ).toBe(0);
    const codeToggle = cell.locator(".ironpad-collapse-default-btn").first();
    await expect(codeToggle).toHaveClass(/ironpad-collapse-default-btn--collapsed/, {
      timeout: 10_000,
    });
    await codeToggle.click();
    // `collapsed: false` is the default, which the cell's JSON omits.
    await expect
      .poll(() => cliExec(["cells", "get", cellId]).collapsed ?? false, {
        timeout: 10_000,
      })
      .toBe(false);
  });

  test("agent updates notebook shared source and cargo", async ({ page }) => {
    test.setTimeout(60_000);

    await createNotebook(page);
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Set both shared fields in one patch.
    const update = cliExecRaw([
      "notebook",
      "update",
      "--shared-source",
      "pub fn from_cli() -> i32 { 9 }",
      "--shared-cargo-toml",
      '[dependencies]\nironpad-cell = "0.1"',
    ]);
    expect(update.exitCode).toBe(0);

    // The daemon's cached copy rebuilds from the browser's mirrored event
    // (not from its own request), so this read asserts the full loop:
    // CLI -> relay -> browser model.apply -> event -> daemon. Polled because
    // the mutation ack can land a beat before the event fan-out.
    await expect
      .poll(() => cliExec(["notebook"]).shared_source ?? "", {
        timeout: 10_000,
      })
      .toContain("from_cli");
    // Both fields ride the same event, so one plain read suffices here.
    expect(cliExec(["notebook"]).shared_cargo_toml).toContain("ironpad-cell");

    // The browser persisted the change through the same path as local edits.
    await expect
      .poll(
        () =>
          page.evaluate(async (id) => {
            const nb = await (window as any).IronpadStorage.getNotebook(id);
            return nb?.shared_source ?? "";
          }, notebookId),
        { timeout: 10_000 },
      )
      .toContain("from_cli");

    // And the editor's shared appendix shows it (the panel seeds from model
    // state when expanded). dispatchEvent rather than click: with a session
    // active, the sticky session toolbar overlaps the scrolled-to header and
    // intercepts pointer events — an overlay quirk, not what this test is
    // about.
    const sourceSection = page
      .locator(".ironpad-editor-shared-appendix .view-only-shared-section")
      .nth(0);
    await sourceSection
      .locator(".view-only-shared-header")
      .dispatchEvent("click");
    await expect(sourceSection.locator(".view-lines")).toContainText(
      "from_cli",
      { timeout: 15_000 },
    );

    // Clearing round-trips to an absent field, not an empty string.
    const clear = cliExecRaw(["notebook", "update", "--clear-shared-source"]);
    expect(clear.exitCode).toBe(0);
    await expect
      .poll(() => cliExec(["notebook"]).shared_source ?? null, {
        timeout: 10_000,
      })
      .toBeNull();
  });

  test("agent runs a cell and reads its output (PRD-0052)", async ({
    page,
  }) => {
    // Compile-scale timeout: the run blocks on a real (possibly cold) build.
    test.setTimeout(240_000);

    const jsErrors = trackJsErrors(page);

    await createNotebook(page);
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Author the cell from the agent side too — the full loop is write,
    // run, read. Same source as execution.spec.ts, so the compile cache is
    // usually warm.
    const addResult = cliExecRaw([
      "cells",
      "add",
      "--source",
      'CellOutput::text(format!("{}", 42))',
      "--label",
      "Agent Run",
    ]);
    expect(addResult.exitCode).toBe(0);
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1, {
      timeout: 15_000,
    });

    const cells = cliExec(["cells", "list"]);
    const cellId = cells[0].id;

    // Run and block until the browser reports the terminal event.
    const result = cliExec(["cells", "run", cellId], 180_000);
    expect(result.status).toBe("executed");
    expect(result.cell_id).toBe(cellId);
    expect(result.display_text).toContain("42");
    expect(result.execution_time_ms).toBeGreaterThanOrEqual(0);

    // The browser really executed it: success badge + rendered output — the
    // CLI result and the page must agree.
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 10_000,
    });
    await expect(cell.locator(".ironpad-output-display-text")).toContainText(
      "42"
    );

    // A cell that doesn't exist is refused up front, not queued into a
    // timeout.
    const bogus = cliExecRaw(["cells", "run", "no-such-cell"]);
    expect(bogus.exitCode).not.toBe(0);

    expect(jsErrors).toEqual([]);
  });

  test("cells run returns a result larger than 1 MiB", async ({ page }) => {
    // An 800x400 Canvas is a ~1.28 MB base64 BMP in the CellExecuted panels.
    // That used to overflow the daemon's 1 MiB IPC frame, so a run that
    // succeeded in the browser came back as "no response from daemon" (exit
    // 4). Only status and exit code are asserted: the result's display text
    // is allowed to change shape.
    test.setTimeout(240_000);

    await createNotebook(page);
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    const addResult = cliExecRaw([
      "cells",
      "add",
      "--source",
      "Canvas::from_fn(800, 400, |x, y| ((x % 256) as u8, (y % 256) as u8, 128))",
      "--label",
      "Big Canvas",
    ]);
    expect(addResult.exitCode).toBe(0);
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1, {
      timeout: 15_000,
    });

    const cellId = cliExec(["cells", "list"])[0].id;
    const run = cliExecRaw(["cells", "run", cellId], 180_000);
    expect(run.stderr).toBe("");
    expect(run.exitCode).toBe(0);
    expect(JSON.parse(run.stdout).status).toBe("executed");
  });

  // A run's whole panels JSON rides `CellExecuted` to agents, and the relay
  // closes a host socket over a frame past 8 MiB, which ends every agent
  // session on the notebook. A 1500x1500 Canvas is ~9 MB of base64, so the
  // event carries a placeholder while the browser still renders the image.
  test("a large output reaches agents as a placeholder and keeps the session", async ({
    page,
  }) => {
    test.setTimeout(240_000);

    const jsErrors = trackJsErrors(page);

    await createNotebook(page);
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    const addResult = cliExecRaw([
      "cells",
      "add",
      "--source",
      "CellOutput::from(Canvas::from_fn(1500, 1500, |x, y| ((x % 256) as u8, (y % 256) as u8, 128)))",
      "--label",
      "Big Canvas",
    ]);
    expect(addResult.exitCode).toBe(0);
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1, {
      timeout: 15_000,
    });
    const cellId = cliExec(["cells", "list"])[0].id;

    const run = cliExecRaw(["cells", "run", cellId], 180_000);
    expect(run.exitCode, run.stderr).toBe(0);
    const result = JSON.parse(run.stdout.trim());
    expect(result.status).toBe("executed");
    expect(result.display_text).toContain("too large");

    // The session survived the run.
    expect(cliExec(["status"]).connected).toBe(true);

    // The browser keeps the full output.
    const cell = page.locator(".ironpad-cell-card").first();
    await expect(cell.locator(".ironpad-output-visual img")).toBeVisible({
      timeout: 10_000,
    });

    expect(jsErrors).toEqual([]);
  });

  test("navigating away ends the session instead of poisoning the page", async ({
    page,
  }) => {
    test.setTimeout(90_000);

    // The regression signature: a leaked WS handler reading the disposed
    // page's signals panics with "already been disposed", which
    // trackJsErrors records as an app panic.
    const jsErrors = trackJsErrors(page);

    await createNotebook(page);
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];
    const token = await startSession(page);
    cliHandle = await connectCli(token);
    expect(cliExec(["status"]).connected).toBe(true);

    // Leave the notebook WITHOUT stopping the session. Pre-fix, the socket
    // and its forget()-leaked handlers survived the page; the next incoming
    // agent message then panicked the wasm runtime and left every page
    // render-only until a hard reload ("cannot edit the title").
    await page.locator("a.ironpad-brand").click();
    await expect(page.locator(".ironpad-home")).toBeVisible({
      timeout: 10_000,
    });

    // Page disposal must end the session server-side: the daemon observes
    // the close and commands fail fast instead of stranding on a timeout.
    await expect
      .poll(() => cliExecRaw(["status"]).exitCode, { timeout: 10_000 })
      .not.toBe(0);
    const late = cliExecRaw([
      "cells",
      "add",
      "--label",
      "probe",
      "--source",
      "42",
    ]);
    expect(late.exitCode).not.toBe(0);

    // The SAME wasm instance must still be fully reactive: SPA-navigate back
    // into the notebook (no reload — a reload would mask a poisoned runtime)
    // and run the title-edit flow end to end.
    await page.locator(`a[href="/local/${notebookId}"]`).click();
    const title = page.locator(".ironpad-notebook-title--editable");
    await expect(title).toBeVisible({ timeout: 15_000 });
    await title.click();
    const input = page.locator(".ironpad-header-title-input");
    await expect(input).toBeVisible();
    await input.fill("");
    await input.pressSequentially("Still Alive", { delay: 20 });
    await input.press("Enter");
    await expect(title).toHaveText("Still Alive", { timeout: 10_000 });

    expect(jsErrors).toEqual([]);
  });

  test("session ends on tab close", async ({ page }) => {
    test.setTimeout(60_000);

    // Create notebook.
    await createNotebook(page);

    // Start session and connect.
    const token = await startSession(page);
    cliHandle = await connectCli(token);

    // Verify connected.
    const status = cliExec(["status"]);
    expect(status.connected).toBe(true);

    // Close the browser tab.
    await page.close();

    // Give daemon time to detect disconnect.
    await new Promise((r) => setTimeout(r, 3_000));

    // Verify CLI commands fail.
    const result = cliExecRaw(["status"]);
    expect(result.exitCode).not.toBe(0);
  });
});

// ── Run-queue ownership: drops are reported, never silent ──────────────────
//
// Each test owns its daemon (a worker has its own temp HOME, see
// helpers/cli.ts), so this block need not be serial; keeping it out of the
// serial block above means one red test cannot skip the others.
//
// Every test here installs a compile gate, so every CLI call after it goes
// through `cliExecRawAsync`: a blocking call would stall the very route
// handler that forwards the page's compile requests.
test.describe("Agent run queue", () => {
  let cliHandle: CliHandle | null = null;

  test.afterEach(async () => {
    if (cliHandle) {
      stopCli(cliHandle);
      cliHandle = null;
    }
  });

  /** A notebook with an agent connected and `sources` added as code cells. */
  async function agentNotebook(page: Page, sources: string[]): Promise<string[]> {
    await createNotebook(page);
    const token = await startSession(page);
    cliHandle = await connectCli(token);
    // Dismiss the session panel: it floats over the cells' side buttons.
    await page.locator(".ironpad-editor").click({ position: { x: 5, y: 5 } });
    await expect(page.locator(".ironpad-session-panel")).toHaveCount(0);
    // In notebook order: `cells add` without `--after` inserts at the TOP,
    // which would reverse every `cellN` reference below.
    const ids: string[] = [];
    for (const [i, source] of sources.entries()) {
      const args = ["cells", "add", "--source", source, "--label", `Q${i}`];
      if (ids.length > 0) args.push("--after", ids[ids.length - 1]);
      expect(cliExecRaw(args).exitCode).toBe(0);
      await expect(page.locator(".ironpad-cell-card")).toHaveCount(i + 1, {
        timeout: 15_000,
      });
      let id: string | undefined;
      await expect
        .poll(() => (id = cliExec(["cells", "list"]).find((c: any) => c.label === `Q${i}`)?.id), {
          timeout: 10_000,
        })
        .toBeTruthy();
      ids.push(id!);
    }
    expect(cliExec(["cells", "list"]).map((c: any) => c.id)).toEqual(ids);
    return ids;
  }

  function parseRun(result: { stdout: string; stderr: string; exitCode: number }) {
    expect(result.exitCode, `cells run failed: ${result.stderr}`).toBe(0);
    return JSON.parse(result.stdout);
  }

  test("Run All Below during an agent's cascade keeps the agent's target", async ({
    page,
  }) => {
    // Review soc-app-client-5: the queue had no owner, and Run All Below
    // REPLACED an in-flight queue, flipping the agent's target from Queued to
    // Idle with no report, so `cells run` waited out its timeout. It must be
    // Run All Below on a cell AFTER the target: Run All from the top
    // re-includes the target and would pass either way.
    test.setTimeout(360_000);
    const jsErrors = trackJsErrors(page);

    const ids = await agentNotebook(page, ["1000u32 + 0", "cell0 + 200", "cell1 + 34", "7u32"]);
    const gate = await gateCompiles(page);

    // A cold cascade: the target's two dependencies queue ahead of it, and
    // the front's compile is held so the click lands mid-run.
    const run = cliExecRawAsync(["cells", "run", ids[2], "--timeout-secs", "150"], 170_000);
    await gate.waitForHeld(1);

    const cards = page.locator(".ironpad-cell-card");
    await page.locator(".ironpad-cell-row").nth(3).locator('button[title="Cell menu"]').click();
    await page.locator(".ironpad-cell-menu-item", { hasText: "Run All Below" }).click();
    gate.releaseAll();

    const result = parseRun(await run);
    expect(result.status).toBe("executed");
    expect(result.display_text).toContain("1234");
    // And the click was honoured too: the later cell ran.
    await expect(cards.nth(3).locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 120_000,
    });
    expect(jsErrors).toEqual([]);
  });

  test("cancelling a compile is terminal for a waiting agent", async ({ page }) => {
    // Review soc-app-client-2: Stop during the compile phase set the cell
    // Idle and nothing else, so an agent that had seen CellCompiling heard
    // nothing more and waited out its whole timeout.
    test.setTimeout(180_000);
    const jsErrors = trackJsErrors(page);

    const [id] = await agentNotebook(page, ["5u32 + 0"]);
    const gate = await gateCompiles(page);

    const run = cliExecRawAsync(["cells", "run", id, "--timeout-secs", "60"], 80_000);
    await gate.waitForHeld(1);
    const card = page.locator(".ironpad-cell-card").first();
    await expect(card.locator(".ironpad-cell-status--compiling")).toBeVisible();

    const clicked = Date.now();
    await page.locator(".ironpad-cell-row").first().locator('button[title="Cancel execution"]').click();
    const result = parseRun(await run);
    expect(result.status).toBe("cancelled");
    expect(Date.now() - clicked).toBeLessThan(30_000);

    // The discarded compile, when it lands, changes nothing.
    gate.releaseAll();
    await page.waitForTimeout(5_000);
    await expect(card.locator(".ironpad-cell-status--idle")).toBeVisible();
    expect(jsErrors).toEqual([]);
  });

  test("switching to Preview mid-run reports the run cancelled", async ({ page }) => {
    // Review bug-editor-18: Preview unmounts every cell, so an in-flight
    // queue had nothing left to drain or report it, and the agent waited out
    // its timeout.
    test.setTimeout(180_000);
    const jsErrors = trackJsErrors(page);

    const [id] = await agentNotebook(page, ["6u32 + 0"]);
    const gate = await gateCompiles(page);

    const run = cliExecRawAsync(["cells", "run", id, "--timeout-secs", "60"], 80_000);
    await gate.waitForHeld(1);

    const clicked = Date.now();
    await page.locator('button[aria-label="Preview"]').click();
    const result = parseRun(await run);
    expect(result.status).toBe("cancelled");
    expect(result.cell_id).toBe(id);
    expect(Date.now() - clicked).toBeLessThan(30_000);

    gate.releaseAll();
    await page.locator('button[aria-label="Edit"]').click();
    expect(jsErrors).toEqual([]);
  });

  test("run after update during an in-flight compile reports the new source", async ({
    page,
  }) => {
    // Review bug-common-cli-6: the in-flight run of the OLD source satisfied
    // the second `cells run`, which then reported the old output as success
    // and cleared the stale flag the update had set.
    test.setTimeout(300_000);
    const jsErrors = trackJsErrors(page);

    const [id] = await agentNotebook(page, ["111u32 + 0"]);
    const gate = await gateCompiles(page);

    const queued = await cliExecRawAsync(["cells", "run", id, "--no-wait"]);
    expect(queued.exitCode).toBe(0);
    await gate.waitForHeld(1);

    const updated = await cliExecRawAsync(["cells", "update", id, "--source", "222u32 + 0"]);
    expect(updated.exitCode).toBe(0);
    const card = page.locator(".ironpad-cell-card").first();
    await expect(card.locator(".monaco-editor").first()).toContainText("222u32", {
      timeout: 10_000,
    });

    // Waiting first, then releasing, so the old compile lands while the
    // second run is listening.
    const run = cliExecRawAsync(["cells", "run", id, "--timeout-secs", "150"], 170_000);
    await page.waitForTimeout(1_000);
    gate.releaseAll();

    const result = parseRun(await run);
    expect(result.status).toBe("executed");
    expect(result.display_text).toContain("222");
    expect(result.display_text).not.toContain("111");
    await expect(card.locator(".ironpad-cell-status--success")).toBeVisible({
      timeout: 60_000,
    });
    await expect(card.locator(".ironpad-stale-indicator")).toHaveCount(0);
    expect(jsErrors).toEqual([]);
  });
});
