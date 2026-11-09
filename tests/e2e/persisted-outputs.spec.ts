import { test, expect, Page } from "@playwright/test";
import { loginTestUser } from "./helpers/auth";
import { setCellSource } from "./helpers/monaco";
import { MENU, shareMutable } from "./helpers/mutable";
import { createNotebook } from "./helpers/session";
import { fixtureCell, notebookFixture, shareFixture } from "./helpers/share";

/**
 * PRD-0056: persisted cell outputs. Sharing embeds the author's last-run
 * panels into the notebook JSON; view-only pages render them as the initial
 * output state — badged as serialized, with Run as the live upgrade — so a
 * fresh reader sees a finished document with zero compiles.
 */


/** Add a cell, set its source, run it, and wait for the output text. */
async function addRunCell(page: Page, source: string, expectText: string) {
  await page.locator(".ironpad-add-cell-btn").first().click();
  const cell = page.locator(".ironpad-cell-card").first();
  await expect(cell.locator(".monaco-editor").first()).toBeVisible({
    timeout: 15_000,
  });
  await setCellSource(page, cell, source);
  await page.locator('button[title="Run cell"]').first().click();
  await expect(cell.locator(".ironpad-cell-status--success")).toBeVisible({
    timeout: 120_000,
  });
  await expect(cell.locator(".ironpad-output-display-text")).toContainText(
    expectText,
    { timeout: 5_000 },
  );
}

test.describe("Persisted cell outputs (PRD-0056)", () => {
  test("a shared notebook renders the saved output cold, and Run replaces it", async ({
    page,
    browser,
  }) => {
    test.setTimeout(240_000);

    await createNotebook(page);
    await addRunCell(page, 'CellOutput::text(format!("{}", 41 + 1))', "42");

    // Share Immutable embeds the saved output into the uploaded JSON.
    await page.locator(MENU).click();
    await page
      .locator(".ironpad-toolbar-dropdown-item", { hasText: "Share Immutable" })
      .click();
    const toast = page.locator(".ironpad-toast-body", { hasText: "/shared/" });
    await expect(toast).toBeVisible({ timeout: 30_000 });
    const url = (await toast.textContent())!.match(
      /\/shared\/[a-f0-9]{16}/,
    )![0];

    // A fresh anonymous context sees the output and the badge with ZERO
    // compiles or runs — the product requirement is that the page says the
    // output is serialized AND that Run executes it live.
    const anon = await browser.newContext();
    try {
      const reader = await anon.newPage();
      await reader.goto(url);
      await expect(reader.locator(".view-only-notebook")).toBeVisible({
        timeout: 30_000,
      });
      const saved = reader.locator(".view-only-output--saved").first();
      await expect(saved).toBeVisible({ timeout: 15_000 });
      await expect(saved.locator(".view-only-saved-badge")).toContainText(
        "Saved output",
      );
      await expect(saved.locator(".view-only-saved-badge")).toContainText(
        "Run to execute live",
      );
      await expect(saved).toContainText("42");

      // Running the cell replaces the snapshot with a live result: the
      // badge goes away and the normal output block (with timing meta)
      // appears. The share carries a blob snapshot, so this is fast.
      await reader.waitForTimeout(3_000); // hydration (suite convention)
      // PRD-0065 made the run control icon-only, so `hasText: "Run"` no longer
      // matches. Select it by role, which is what an icon-only button needs
      // anyway and what the accessible name is bound to.
      await reader
        .getByRole("button", { name: "Run this cell" })
        .first()
        .click();
      await expect(reader.locator(".view-only-output--saved")).toHaveCount(0, {
        timeout: 120_000,
      });
      await expect(
        reader.locator(".view-only-output .view-only-output-caption").first(),
      ).toBeVisible({ timeout: 15_000 });
      await expect(reader.locator(".view-only-output").first()).toContainText(
        "42",
      );
    } finally {
      await anon.close();
    }
  });

  test("publish and push both carry the outputs to /mutable readers", async ({
    page,
    browser,
  }) => {
    test.setTimeout(240_000);
    await loginTestUser(page, "alice");
    await createNotebook(page);
    await addRunCell(page, 'CellOutput::text(format!("{}", 41 + 1))', "42");

    // Share Mutable: the create path embeds outputs like Share Immutable.
    const shareId = await shareMutable(page);

    const readerSeesSaved = async () => {
      const anon = await browser.newContext();
      try {
        const reader = await anon.newPage();
        await reader.goto(`/mutable/${shareId}`);
        const saved = reader.locator(".view-only-output--saved").first();
        await expect(saved).toBeVisible({ timeout: 30_000 });
        await expect(saved).toContainText("42");
      } finally {
        await anon.close();
      }
    };
    await readerSeesSaved();

    // Now the PUSH path (uat-003): dirty the draft with a rename (a lean
    // 1.5s autosave writes it WITHOUT outputs), then Push. The pre-push
    // durable save must re-embed the outputs — if it saved lean, the
    // promote would publish a draft with no saved_output and the reader's
    // snapshot below would vanish.
    await page.locator(".ironpad-notebook-title--editable").click();
    const input = page.locator(".ironpad-header-title-input");
    await input.fill("Pushed With Outputs");
    await input.press("Enter");
    const push = page.locator(".ironpad-push-button");
    await expect(push).toBeEnabled({ timeout: 10_000 });
    await page.waitForTimeout(3_000); // let the lean autosave land first
    await push.click();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Pushed" }),
    ).toBeVisible({ timeout: 30_000 });

    await readerSeesSaved();
  });
});

/**
 * A share whose FIRST cell carries `saved_output`, hard-loaded by a fresh
 * reader. Returns the reader page (its context closes with `anon`) and every
 * raw `pageerror`, unfiltered: `trackJsErrors` drops messages containing
 * "unreachable", which is exactly the hydration panic these guard against.
 */
async function readSavedFirstCell(
  page: Page,
  browser: import("@playwright/test").Browser,
  savedOutput: unknown,
) {
  const url = await shareFixture(
    page,
    notebookFixture("Saved panel", [
      fixtureCell("saved", 0, "Code", "CellOutput::empty()", savedOutput),
      fixtureCell("after", 1, "Code", 'CellOutput::text("after")'),
    ]),
  );
  const anon = await browser.newContext();
  const reader = await anon.newPage();
  const errors: string[] = [];
  reader.on("pageerror", (e) => errors.push(e.message));
  await reader.goto(url);
  await expect(reader.locator(".view-only-output--saved").first()).toBeVisible({
    timeout: 30_000,
  });
  await reader.waitForTimeout(3_000); // hydration (suite convention)
  return { anon, reader, errors };
}

/**
 * Hydration reached past the saved panel: the SECOND cell's chevron works.
 * The first cell hydrates before a panic in its output, so its own chevron
 * would prove nothing.
 */
async function expectSecondCellHydrated(reader: Page) {
  const second = reader.locator(".view-only-cell--frame").nth(1);
  const body = second.locator(".ironpad-cell-body").first();
  await expect(body).not.toHaveClass(/ironpad-cell-body--collapsed/);
  await second.locator(".ironpad-cell-collapse-btn").click();
  await expect(body).toHaveClass(/ironpad-cell-body--collapsed/, { timeout: 5_000 });
}

test.describe("Saved panels hydrate on a hard load", () => {
  for (const [name, panel] of [
    ["Animation", { Animation: { width: 1, height: 1, fps: 1, frame_count: 1, data: "AAAA" } }],
    [
      "Simulation",
      { Simulation: { width: 1, height: 1, fps: 1, first_frame_data: "AAAA", sliders: [] } },
    ],
  ] as const) {
    test(`a saved ${name} panel renders the same DOM on both sides`, async ({
      page,
      browser,
    }) => {
      test.setTimeout(120_000);
      const { anon, reader, errors } = await readSavedFirstCell(page, browser, [panel]);
      try {
        await expect(reader.locator(".view-only-output--saved canvas")).toHaveCount(1);
        await expectSecondCellHydrated(reader);
        expect(errors, "no hydration panic").toEqual([]);
      } finally {
        await anon.close();
      }
    });
  }

  test("a saved BlobImage panel shows its image, not a broken one", async ({
    page,
    browser,
  }) => {
    test.setTimeout(120_000);
    const png =
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M/wHwAEBgIApD5fRAAAAABJRU5ErkJggg==";
    const { anon, reader, errors } = await readSavedFirstCell(page, browser, [
      { BlobImage: { mime_type: "image/png", base64_data: png, width: 1, height: 1 } },
    ]);
    try {
      const img = reader.locator(".view-only-output-visual img").first();
      await expect
        .poll(() => img.evaluate((el) => (el as HTMLImageElement).naturalWidth), {
          timeout: 10_000,
        })
        .toBeGreaterThan(0);
      expect(errors).toEqual([]);
    } finally {
      await anon.close();
    }
  });
});
