import { Page, expect } from "@playwright/test";
import { menuClick } from "./menu";

/**
 * Import a notebook fixture into IndexedDB, open it in the editor, Share
 * Immutable, and return the `/shared/{hash}` path.
 *
 * The one home for seeding a share from a hand-built fixture: the fixture's
 * `saved_output` survives the share because a session with no fresh capture
 * preserves prior snapshots (PRD-0056), so a spec can put any panel in front
 * of a reader without compiling a cell.
 */
export async function shareFixture(page: Page, fixture: object): Promise<string> {
  await page.goto("/");
  await expect(page.locator(".ironpad-home")).toBeVisible({ timeout: 15_000 });
  const imported = await page.evaluate(
    (json) =>
      (
        window as unknown as {
          IronpadStorage: { importNotebook: (j: string) => Promise<{ id: string }> };
        }
      ).IronpadStorage.importNotebook(json),
    JSON.stringify(fixture),
  );
  await page.goto(`/local/${imported.id}`);
  await expect(page.locator(".ironpad-editor")).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(3_000); // hydration (suite convention)
  await menuClick(page, "Share Immutable");
  const toast = page.locator(".ironpad-toast-body", { hasText: "/shared/" });
  await expect(toast).toBeVisible({ timeout: 30_000 });
  return (await toast.textContent())!.match(/\/shared\/[a-f0-9]{16}/)![0];
}

/** A cell of `cell_type` for a hand-built notebook fixture. */
export function fixtureCell(
  id: string,
  order: number,
  cell_type: "Code" | "Markdown",
  source: string,
  saved_output?: unknown,
) {
  return {
    id,
    order,
    label: id,
    cell_type,
    source,
    ...(cell_type === "Code" ? { cargo_toml: "" } : {}),
    ...(saved_output === undefined ? {} : { saved_output: JSON.stringify(saved_output) }),
  };
}

/** A minimal notebook fixture around `cells`. */
export function notebookFixture(title: string, cells: object[]) {
  return {
    version: 1,
    id: "00000000-0000-0000-0000-000000000000",
    title,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    shared_cargo_toml: "",
    cells,
  };
}
