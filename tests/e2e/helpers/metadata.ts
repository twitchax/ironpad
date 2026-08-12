import { Locator, Page, expect } from "@playwright/test";

/**
 * The notebook metadata panel (PRD-0051), and the ONE way to open it.
 *
 * The panel is collapsed by default and mounts its body lazily, so every
 * spec that touches it needs the same three steps. They were hand-rolled in
 * two places already (the metadata spec and the private-shares spec, whose
 * copy waited on the Access controls instead of the body); PRD-0064 makes
 * the panel's contents CONDITIONAL on whether the notebook is published, so
 * a third divergent copy would be a copy that can silently wait for the
 * wrong thing.
 *
 * Waits on the description textarea rather than on anything conditional: it
 * is the one field the panel renders in every state.
 */
export async function openMetadataPanel(page: Page): Promise<Locator> {
  const section = page.locator(
    ".ironpad-editor-metadata-appendix .view-only-shared-section",
  );
  await expect(section).toBeVisible({ timeout: 15_000 });
  await section.locator(".view-only-shared-header").click();
  await expect(section.locator(".ironpad-metadata-textarea")).toBeVisible({
    timeout: 15_000,
  });
  return section;
}
