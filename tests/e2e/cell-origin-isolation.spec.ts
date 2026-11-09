import { test, expect, Page, Request } from "@playwright/test";
import { loginTestUser } from "./helpers/auth";
import { menuClick } from "./helpers/menu";
import { openMetadataPanel } from "./helpers/metadata";
import { shareMutable } from "./helpers/mutable";
import { createNotebook, renameNotebook } from "./helpers/session";

/**
 * Cell confinement (review bug-db-auth-1, Step 1).
 *
 * Cells run in a same-origin worker, so before this a stranger's /shared
 * notebook ran with the viewer's session: every request a cell made carried
 * the session cookie, the worker could open the origin's IndexedDB, and a
 * cell that crashed in the worker was re-run on the main thread, where none
 * of the worker's confinement applies. The worker now strips credentials
 * from fetch and removes IndexedDB, Cache Storage and XMLHttpRequest
 * (public/executor-confine.js), and the untrusted pages refuse the
 * main-thread fallback.
 *
 * This is a speed bump, not a boundary: a cell's own `new Worker(blob)`
 * still gets an unconfined scope. The boundary is a separate cell origin.
 *
 * The victim is signed in and owns a PRIVATE published notebook whose title
 * is the sentinel, so both a GET of its reader page and a POST to
 * list_mutable_shares answer with the sentinel only when the request carries
 * the owner's cookie. The positive controls make the same requests from the
 * page's main thread and must see it.
 */

const SENTINEL = "cellconf-sentinel-5d1e";
const FORK_HINT = "fork the notebook to run it";

/** One code cell of the attacker's notebook. */
function codeCell(id: string, label: string, source: string) {
  return { id, order: 0, label, cell_type: "Code", source, cargo_toml: "" };
}

/**
 * The attacker's cells. Each reports a verdict rather than the response, so
 * a pass is `leaked=false` and a failure names the vector that leaked.
 */
function attackCells(shareId: string, list: { url: string; contentType: string }) {
  const leakedExpr = (v: string) =>
    `${v}.contains("${SENTINEL}") || ${v}.contains("${shareId}")`;
  const listFetch =
    `fetch(${JSON.stringify(list.url)}, {method: "POST", ` +
    `headers: {"Content-Type": ${JSON.stringify(list.contentType)}}, body: ""})` +
    `.then(r => r.text())`;
  return [
    // ironpad_cell::http is reqwest, which needs an absolute URL.
    codeCell(
      "c-http",
      "via-http",
      [
        `let origin = js_sys::eval("self.location.origin").ok().and_then(|v| v.as_string()).unwrap_or_default();`,
        `let body = ironpad_cell::http::get(&format!("{origin}/mutable/${shareId}")).await;`,
        `let body = body.unwrap_or_else(|e| format!("request failed: {e}"));`,
        `format!("http reached={} leaked={}", !body.starts_with("request failed"), ${leakedExpr("body")})`,
      ].join("\n"),
    ),
    // Fully synchronous (a JSPI cell must not contain the await keyword).
    codeCell(
      "c-blocking",
      "via-blocking",
      [
        `let r = blocking::fetch_text("/mutable/${shareId}");`,
        `let leaked = r.as_ref().map(|b| ${leakedExpr("b")}).unwrap_or(false);`,
        `let status = r.as_ref().map(|_| "ok".to_string()).unwrap_or_else(|e| e.clone());`,
        `format!("blocking status={status} leaked={leaked}")`,
      ].join("\n"),
    ),
    codeCell(
      "c-eval",
      "via-eval",
      [
        `let promise = js_sys::Promise::from(js_sys::eval(${JSON.stringify(listFetch)}).unwrap());`,
        `let text = wasm_bindgen_futures::JsFuture::from(promise).await;`,
        `let text = text.map(|v| v.as_string().unwrap_or_default()).unwrap_or_else(|e| format!("{e:?}"));`,
        `format!("eval reached={} leaked={}", !text.is_empty(), ${leakedExpr("text")})`,
      ].join("\n"),
    ),
    codeCell(
      "c-storage",
      "storage-probe",
      [
        `let probe = |expr: &str| js_sys::eval(expr).ok().and_then(|v| v.as_string()).unwrap_or_default();`,
        `format!("indexedDB={} caches={} XMLHttpRequest={}", probe("typeof indexedDB"), probe("typeof caches"), probe("typeof XMLHttpRequest"))`,
      ].join("\n"),
    ),
    // Crashes only in the worker, so it succeeds exactly when it is re-run
    // on the main thread.
    codeCell(
      "c-crash",
      "worker-crash",
      [
        `let in_worker = js_sys::eval("typeof WorkerGlobalScope !== 'undefined'").ok().and_then(|v| v.as_bool()).unwrap_or(false);`,
        `if in_worker {`,
        `    panic!("refusing to run inside the worker");`,
        `}`,
        `String::from("ran on the main thread")`,
      ].join("\n"),
    ),
  ];
}

/**
 * A view-only cell's result once it settles: its output text or its error,
 * never its source (which names the sentinel, the share id and the main-thread
 * string the crash cell returns).
 */
async function viewerResult(page: Page, label: string): Promise<string> {
  const cell = page.locator(".view-only-cell", {
    has: page.locator(".view-only-cell-label", { hasText: label }),
  });
  const settled = cell.locator(".view-only-error, .view-only-output-text");
  await expect(settled.first()).toBeVisible({ timeout: 300_000 });
  return (await settled.allInnerTexts()).join(" ").replace(/\s+/g, " ");
}

test.describe("Cell confinement (review bug-db-auth-1)", () => {
  test("a stranger's cell cannot use the viewer's session, storage or main thread", async ({
    page,
    browser,
  }) => {
    test.setTimeout(600_000);

    // ── Victim: signed in, owning a private notebook titled SENTINEL ──────
    await loginTestUser(page, "cellconf-victim");
    await createNotebook(page);
    await renameNotebook(page, SENTINEL);
    const shareId = await shareMutable(page);
    await openMetadataPanel(page);
    await expect(page.locator(".ironpad-access-toggle")).toBeVisible({
      timeout: 15_000,
    });
    await page.locator(".ironpad-access-toggle input").check();
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "private" }),
    ).toBeVisible({ timeout: 15_000 });

    // The home page's own list_mutable_shares call supplies the endpoint and
    // the encoding, so the cells make exactly the request the app makes.
    const listRequest: Promise<Request> = page.waitForRequest((r) =>
      r.url().includes("list_mutable_shares"),
    );
    await page.goto("/");
    const req = await listRequest;
    const list = {
      url: new URL(req.url()).pathname,
      contentType: req.headers()["content-type"] ?? "application/x-www-form-urlencoded",
    };

    // Positive control: from the main thread, both requests carry the
    // cookie and answer with the sentinel.
    const control = await page.evaluate(
      async ({ shareId, list }) => {
        const reader = await (await fetch(`/mutable/${shareId}`)).text();
        const listed = await (
          await fetch(list.url, {
            method: "POST",
            headers: { "Content-Type": list.contentType },
            body: "",
          })
        ).text();
        return { reader, listed };
      },
      { shareId, list },
    );
    expect(control.reader).toContain(SENTINEL);
    expect(control.listed).toContain(SENTINEL);
    expect(control.listed).toContain(shareId);

    // ── Attacker: an anonymous author shares the notebook ─────────────────
    const attacker = await browser.newContext();
    let sharedHash: string;
    let localId: string;
    try {
      const author = await attacker.newPage();
      await author.goto("/");
      await expect(author.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
      const fixture = {
        version: 1,
        id: "00000000-0000-0000-0000-ce11c0f1e001",
        title: "Totally normal notebook",
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-01T00:00:00Z",
        shared_cargo_toml: "",
        cells: attackCells(shareId, list).map((c, i) => ({ ...c, order: i })),
      };
      const imported = await author.evaluate(
        (json) => (window as any).IronpadStorage.importNotebook(json),
        JSON.stringify(fixture),
      );
      localId = imported.id;
      await author.goto(`/local/${localId}`);
      await expect(author.locator(".ironpad-editor")).toBeVisible({ timeout: 15_000 });
      await author.waitForTimeout(3_000); // hydration (suite convention)

      // Shared before anything runs, so no saved output rides along.
      await menuClick(author, "Share Immutable");
      const toastBody = author.locator(".ironpad-toast-body", { hasText: "/shared/" });
      await expect(toastBody).toContainText("/shared/", { timeout: 30_000 });
      sharedHash = (await toastBody.textContent())!.match(/\/shared\/([0-9a-f]{16})/)![1];

      // Positive control for the fallback: in the author's own editor the
      // worker-only crash is retried on the main thread, and succeeds there.
      // The editor's Run button sits beside the card, not inside it.
      const crashCell = author.locator(".ironpad-cell-card").nth(4);
      await author
        .locator(".ironpad-cell-side-actions")
        .nth(4)
        .locator('button[title="Run cell"]')
        .click({ timeout: 30_000 });
      await expect(crashCell.locator(".ironpad-output-display-text")).toContainText(
        "ran on the main thread",
        { timeout: 300_000 },
      );
      await expect(crashCell.locator(".ironpad-output-fallback-badge")).toBeVisible();
    } finally {
      await attacker.close();
    }

    // ── Victim opens the shared notebook and runs every cell ──────────────
    await page.goto(`/shared/${sharedHash}`);
    await expect(page.locator(".view-only-notebook")).toBeVisible({ timeout: 30_000 });
    await page.waitForTimeout(3_000); // hydration (suite convention)
    // No cell consumes another, so each Run is a direct, concurrent run.
    const runButtons = page.locator(".view-only-run-button");
    await expect(runButtons).toHaveCount(5);
    for (let i = 0; i < 5; i++) {
      await runButtons.nth(i).click({ timeout: 30_000 });
    }

    // Soft, so one run reports every vector that leaks rather than the first.
    const http = await viewerResult(page, "via-http");
    const blocking = await viewerResult(page, "via-blocking");
    const evalled = await viewerResult(page, "via-eval");
    for (const text of [http, blocking, evalled]) {
      expect.soft(text).toContain("leaked=false");
    }
    // The uncredentialed requests did reach the server.
    expect.soft(http).toContain("http reached=true");
    expect.soft(evalled).toContain("eval reached=true");

    // Nothing the cells reported carries the victim's data. Scoped to the
    // outputs: the sentinel and share id are in the cells' own source.
    const outputs = (
      await page.locator(".view-only-output-text, .view-only-error").allInnerTexts()
    ).join("\n");
    expect.soft(outputs).not.toContain(SENTINEL);
    expect.soft(outputs).not.toContain(shareId);

    const storage = await viewerResult(page, "storage-probe");
    expect.soft(storage).toContain(
      "indexedDB=undefined caches=undefined XMLHttpRequest=undefined",
    );

    // The worker-only crash is reported, with the way out, and was never
    // re-run on the main thread.
    const crash = await viewerResult(page, "worker-crash");
    expect(crash).toContain(FORK_HINT);
    expect(crash).not.toContain("ran on the main thread");
    await expect(page.locator(".ironpad-output-fallback-badge")).toHaveCount(0);
  });
});
