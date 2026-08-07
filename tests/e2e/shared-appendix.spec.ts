import { test, expect } from "@playwright/test";
import { setCellSource } from "./helpers/monaco";
import { createNotebook as newNotebook } from "./helpers/session";

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
});
