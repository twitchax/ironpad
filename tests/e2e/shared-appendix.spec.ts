import { test, expect, Page } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { createNotebook as newNotebook } from "./helpers/session";

/** The Shared Source section. */
const sharedSource = (page: Page) =>
  page.locator(".ironpad-editor-shared-appendix .view-only-shared-section").nth(0);

/** Expand Shared Source and wait for its editor. */
async function openSharedSource(page: Page) {
  const section = sharedSource(page);
  await section.locator(".view-only-shared-header").click();
  await expect(section.locator(".monaco-editor").first()).toBeVisible({
    timeout: 15_000,
  });
  return section;
}

/** The open local notebook's stored shared source, or null. */
function storedSharedSource(page: Page): Promise<string | null> {
  const id = page.url().match(/\/local\/([a-f0-9-]+)/)![1];
  return page.evaluate(
    async (nid) =>
      (await (window as any).IronpadStorage.getNotebook(nid))?.shared_source ??
      null,
    id,
  );
}

/**
 * Shared source / dependencies appendix in the editor.
 *
 * Notebook-level shared code is edited in collapsible sections below the
 * cell list (mirroring the public/shared view pages) instead of the old
 * gear-menu panels above it. In view mode the sections are read-only and
 * hidden entirely when the field is empty.
 */

test.describe("Shared appendix (editor)", () => {
  test("appendix sections render below the cells and expand to an editor", async ({
    page,
  }) => {
    await newNotebook(page);

    const appendix = page.locator(".ironpad-editor-shared-appendix");
    await expect(appendix).toBeVisible();
    const headers = appendix.locator(".view-only-shared-header");
    await expect(headers).toHaveCount(2);
    await expect(headers.nth(0)).toContainText("Shared Source");
    await expect(headers.nth(1)).toContainText("Shared Dependencies");

    // The gear menu no longer hosts the shared panels.
    await page.locator('button[title="Notebook settings"]').click();
    const gearMenu = page.locator(".ironpad-toolbar-dropdown-menu");
    await expect(gearMenu).toBeVisible();
    await expect(gearMenu).not.toContainText("Shared Source");
    await expect(gearMenu).not.toContainText("Shared Deps");

    // Expanding shared source lazily mounts an editable Monaco with a Save
    // action. (This click is also an outside click that closes the menu.)
    await headers.nth(0).click();
    const sourceSection = appendix.locator(".view-only-shared-section").nth(0);
    await expect(sourceSection.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await expect(
      sourceSection.locator("button", { hasText: "Save" })
    ).toBeVisible();
  });

  test("saved shared source persists across reload", async ({ page }) => {
    await newNotebook(page);
    const url = page.url();

    const sourceSection = page
      .locator(".ironpad-editor-shared-appendix .view-only-shared-section")
      .nth(0);
    await sourceSection.locator(".view-only-shared-header").click();
    await expect(sourceSection.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });

    await setCellSource(
      page,
      sourceSection,
      "pub fn from_appendix() -> i32 { 7 }"
    );
    await sourceSection.locator("button", { hasText: "Save" }).click();

    // The Saving… state is floored at 500ms (the IndexedDB write alone is
    // imperceptibly fast), so it must be observable before the toast.
    await expect(
      sourceSection.locator("button", { hasText: "Saving…" })
    ).toBeVisible();
    await expect(
      page.locator("text=Shared source saved").first()
    ).toBeVisible({ timeout: 10_000 });

    // The toast now means "durably saved", but keep the belt-and-suspenders
    // poll so this test never races persistence again.
    const notebookId = url.match(/\/local\/([a-f0-9-]+)/)![1];
    await expect
      .poll(
        () =>
          page.evaluate(async (id) => {
            const nb = await (window as any).IronpadStorage.getNotebook(id);
            return nb?.shared_source ?? "";
          }, notebookId),
        { timeout: 10_000 }
      )
      .toContain("from_appendix");

    // Reload from IndexedDB and verify the content survived.
    await page.goto(url);
    await expect(page.locator(".ironpad-editor")).toBeVisible({
      timeout: 15_000,
    });
    const reloaded = page
      .locator(".ironpad-editor-shared-appendix .view-only-shared-section")
      .nth(0);
    await reloaded.locator(".view-only-shared-header").click();
    await expect(reloaded.locator(".view-lines")).toContainText(
      "from_appendix",
      { timeout: 15_000 }
    );
  });

  test("view mode hides empty sections and shows content-bearing ones read-only", async ({
    page,
  }) => {
    await newNotebook(page);

    // A fresh notebook has no shared source but does carry the default
    // shared Cargo.toml, so view mode (the public renderer, exactly) shows
    // one deps section in ITS appendix — the content-bearing rule.
    await page.locator('button[aria-label="Preview"]').click();
    const appendix = page.locator(".view-only-shared-appendix");
    await expect(appendix).toBeVisible({ timeout: 10_000 });
    const headers = appendix.locator(".view-only-shared-header");
    await expect(headers).toHaveCount(1);
    await expect(headers.nth(0)).toContainText("Shared Dependencies");

    // Expanded in view mode: Monaco is read-only and there is no Save.
    await headers.nth(0).click();
    const section = appendix.locator(".view-only-shared-section").first();
    await expect(section.locator(".monaco-editor").first()).toBeVisible({
      timeout: 15_000,
    });
    await expect(section.locator("button", { hasText: "Save" })).toHaveCount(
      0
    );

    const readOnly = await page.evaluate(() => {
      const monaco = (window as any).monaco;
      const el = document.querySelector(".view-only-shared-appendix");
      const editor = monaco.editor
        .getEditors()
        .find((e: any) => el!.contains(e.getDomNode()));
      return editor.getOption(monaco.editor.EditorOption.readOnly);
    });
    expect(readOnly).toBe(true);
  });

  // Text typed without the section's Save lived only in the panel: Ctrl+S
  // claimed it saved, Preview rebuilt the panel from the model and a
  // collapse unmounted it, and either way the text was gone.
  test("Ctrl+S saves typed shared source", async ({ page }) => {
    await newNotebook(page);
    const section = await openSharedSource(page);
    await setCellSource(page, section, "pub fn via_ctrl_s() -> i32 { 1 }");
    await page.keyboard.press("Control+s");
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Notebook saved" }),
    ).toBeVisible({ timeout: 10_000 });
    await expect
      .poll(() => storedSharedSource(page), { timeout: 10_000 })
      .toContain("via_ctrl_s");
  });

  test("Preview keeps typed shared source", async ({ page }) => {
    await newNotebook(page);
    const section = await openSharedSource(page);
    await setCellSource(page, section, "pub fn via_preview() -> i32 { 2 }");
    await page.locator('button[aria-label="Preview"]').click();
    await expect(page.locator(".view-only-notebook")).toBeVisible({
      timeout: 10_000,
    });
    await page.locator('button[aria-label="Edit"]').click();
    const again = await openSharedSource(page);
    await expect(again.locator(".view-lines")).toContainText("via_preview", {
      timeout: 15_000,
    });
  });

  test("collapsing keeps typed shared source", async ({ page }) => {
    await newNotebook(page);
    const section = await openSharedSource(page);
    await setCellSource(page, section, "pub fn via_collapse() -> i32 { 3 }");
    await section.locator(".view-only-shared-header").click();
    await expect(section.locator(".monaco-editor")).toHaveCount(0);
    const again = await openSharedSource(page);
    await expect(again.locator(".view-lines")).toContainText("via_collapse", {
      timeout: 15_000,
    });
    await expect
      .poll(() => storedSharedSource(page), { timeout: 10_000 })
      .toContain("via_collapse");
  });

  test("opening and closing the section writes no template", async ({
    page,
  }) => {
    // The negative half: only typed text commits, never the template the
    // editor is prefilled with.
    await newNotebook(page);
    const section = await openSharedSource(page);
    await page.keyboard.press("Control+s");
    await expect(
      page.locator(".ironpad-toast-title", { hasText: "Notebook saved" }),
    ).toBeVisible({ timeout: 10_000 });
    expect(await storedSharedSource(page)).toBeNull();
    await section.locator(".view-only-shared-header").click();
    await expect(section.locator(".monaco-editor")).toHaveCount(0);
    await page.waitForTimeout(1_500);
    expect(await storedSharedSource(page)).toBeNull();
  });
});
