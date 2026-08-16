import { test, expect } from "@playwright/test";
import { POD_HOST, recordPodRequests } from "./helpers/browserpod";
import { trackJsErrors } from "./helpers/errors";
import { ADD_CODE, ADD_LINUX, createNotebook, waitForPersistedCells } from "./helpers/session";

/**
 * PRD-0066: Linux cells — the half that costs nothing to test.
 *
 * Every one of these asserts an ABSENCE (no request to the BrowserPod CDN, no
 * pod, no boot), which is exactly why they belong in the default gate: they
 * spend no metered allowance. The specs that need a real pod live in
 * `tests/e2e/linux-pod/` and run only under `cargo make test-linux-cells`.
 */
test.describe("Linux cells: the CDN stays untouched (PRD-0066)", () => {
  test("the default gate cannot reach the BrowserPod CDN at all", async ({
    page,
  }) => {
    // The meta-guard. `playwright.config.ts` launches this project with the
    // host mapped to ~NOTFOUND so no spec can spend a pod boot by accident,
    // and a guard nobody checks is a guard that quietly stops working.
    //
    // Assert the RESOLVER's error text, not that the fetch rejected. From
    // inside the page a rejection reads `TypeError: Failed to fetch` whether
    // the cause was DNS or the COEP policy this origin already carries, so
    // the obvious version of this test would pass with the flag removed and
    // prove nothing. `net::ERR_NAME_NOT_RESOLVED` can only come from the
    // resolver rule — measured against a control, where the same request
    // succeeds and raises no failure event at all.
    const failures: string[] = [];
    page.on("requestfailed", (req) => {
      if (new URL(req.url()).hostname === POD_HOST) {
        failures.push(req.failure()?.errorText ?? "");
      }
    });

    await page.goto("/");
    await page.evaluate(
      (host) =>
        fetch(`https://${host}/3.0.0/rust/install.sh`, {
          mode: "no-cors",
        }).catch(() => {}),
      POD_HOST,
    );
    await expect
      .poll(() => failures, { timeout: 15_000 })
      .toContain("net::ERR_NAME_NOT_RESOLVED");
  });

  test("a notebook with no Linux cell contacts the CDN on no surface (uat-004)", async ({
    page,
  }) => {
    test.setTimeout(120_000);
    const podRequests = recordPodRequests(page);
    const jsErrors = trackJsErrors(page);

    // The home page, a fresh editor with an ordinary Code cell, and a public
    // notebook — the last one because it AUTORUNS, which is the surface where
    // an accidental boot would be both unattended and repeated per visitor.
    await createNotebook(page);
    await page.locator(ADD_CODE).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);

    await page.goto("/public/welcome");
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 30_000,
    });
    await page.waitForTimeout(5_000); // let autorun get going

    expect(podRequests).toEqual([]);
    expect(jsErrors).toEqual([]);
  });

  test("adding a Linux cell writes CellType::Linux and boots nothing (T-010, uat-005)", async ({
    page,
  }) => {
    test.setTimeout(120_000);
    const podRequests = recordPodRequests(page);
    const jsErrors = trackJsErrors(page);

    await createNotebook(page);
    await page.locator(ADD_LINUX).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await waitForPersistedCells(page, 1);

    // Assert the persisted TYPE, not the rendering: the button's whole job is
    // to mint a cell with a different execution model, and a cell that looks
    // right in the DOM while persisting as `Code` would compile to the wrong
    // target and fail incomprehensibly (the PRD-0047 lesson, restated in
    // CellType's own doc comment).
    const notebookId = page.url().match(/\/local\/([a-f0-9-]+)/)![1];
    const cellTypes = await page.evaluate(async (id) => {
      const nb = await (window as any).IronpadStorage.getNotebook(id);
      return nb.cells.map((c: { cell_type?: string }) => c.cell_type ?? "Code");
    }, notebookId);
    expect(cellTypes).toEqual(["Linux"]);

    // Reload: a Linux cell sitting in an open notebook must still boot
    // nothing. Nothing autoruns one, by construction (PRD-0066 T-008), and
    // the only thing that may ever start a pod is a click on its Run button.
    //
    // The assertion is ZERO requests, not "no kernel.wasm" — deliberately
    // stricter than "no boot". Eagerly importing the SDK because a notebook
    // merely CONTAINS a Linux cell buys nothing: the import is 425ms cold and
    // 6ms warm out of the 1.5-2s a first click pays anyway, and it turns
    // opening a document into third-party traffic. Load it on the click.
    await page.reload();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await page.waitForTimeout(5_000);

    expect(podRequests).toEqual([]);
    expect(jsErrors).toEqual([]);
  });

  test("a Linux cell says where it runs, and no other cell does (T-010)", async ({
    page,
  }) => {
    test.setTimeout(120_000);
    const jsErrors = trackJsErrors(page);
    const NOTICE = ".ironpad-cell-linux-notice";

    await createNotebook(page);

    // A Code cell has a Run button and needs no explanation.
    await page.locator(ADD_CODE).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(1);
    await expect(page.locator(NOTICE)).toHaveCount(0);

    await page.locator(ADD_LINUX).first().click();
    await expect(page.locator(".ironpad-cell-card")).toHaveCount(2);

    // Exactly one notice, on the Linux cell only. The count is the half that
    // matters: a notice rendered on every cell would still make the first
    // assertion pass if it were written as "is visible".
    await expect(page.locator(NOTICE)).toHaveCount(1);
    await expect(page.locator(NOTICE)).toBeVisible();

    // It must name the control that exists. The badge tooltip this replaced
    // said "view mode", and that control was renamed to Preview precisely
    // because "View mode" beside "View as Reader" read as two names for one
    // thing — so the sentence sent an author looking for a button by a name
    // nothing in the UI uses.
    await expect(page.locator(NOTICE)).toContainText("Preview");
    await expect(
      page.locator('.ironpad-mode-toggle button[aria-label="Preview"]'),
    ).toBeVisible();

    // Identify the two cards by CONTENT, not by index. The add-cell row above
    // the list inserts at the TOP, so `ADD_LINUX.first()` put the Linux cell
    // at row 0 and the Code cell at row 1 — the reverse of the click order.
    // An earlier draft of this test used `.nth(1)` for the Linux card and so
    // collapsed the Code cell, then asserted the notice was still visible;
    // it passed, because the Linux notice it was reading had never been
    // collapsed. Same shape as the theme-toggle spec that measured the wrong
    // element for a release. Index is not identity.
    const badge = ".ironpad-cell-type-badge--linux";
    const linuxCard = page
      .locator(".ironpad-cell-row")
      .filter({ has: page.locator(badge) });
    const codeCard = page
      .locator(".ironpad-cell-row")
      .filter({ hasNot: page.locator(badge) });
    await expect(linuxCard).toHaveCount(1);
    await expect(codeCard).toHaveCount(1);

    // Outside the collapsible body, structurally. This is the requirement,
    // and it is asserted directly rather than through the collapse, because
    // `toBeVisible()` CANNOT see the difference: collapse is `max-height: 0;
    // opacity: 0` with `overflow: hidden` on the parent, which clips the
    // child visually while leaving its bounding box intact, and Playwright
    // counts neither zero opacity nor an overflow-clipped box as hidden. A
    // draft of this test moved the notice inside the body as a control and
    // the collapse assertion stayed green.
    await expect(
      linuxCard.locator(`.ironpad-cell-body ${NOTICE}`),
    ).toHaveCount(0);

    // And behaviourally, since "not a descendant" is only a proxy for "the
    // reader can still read it". `checkVisibility` with `opacityProperty`
    // walks the ancestor chain and is the one API here that reports false
    // for a subtree faded to zero.
    await linuxCard.locator(".ironpad-cell-collapse-btn").click();
    await expect(
      linuxCard.locator(".ironpad-cell-body--collapsed"),
    ).toHaveCount(1);
    await expect
      .poll(() =>
        page
          .locator(NOTICE)
          .evaluate((el) =>
            el.checkVisibility({
              opacityProperty: true,
              visibilityProperty: true,
            }),
          ),
      )
      .toBe(true);

    // The notice exists because the Run button does not. Assert the pairing,
    // so removing one without the other is a failure rather than a papercut.
    await expect(linuxCard.locator('button[title="Run cell"]')).toHaveCount(0);
    await expect(codeCard.locator('button[title="Run cell"]')).toHaveCount(1);

    expect(jsErrors).toEqual([]);
  });
});
