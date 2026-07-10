/**
 * Helpers for driving the Monaco editor from Playwright tests.
 */
import { Page, Locator } from "@playwright/test";

/** Set a cell's Monaco editor content via the Monaco API. */
export async function setCellSource(page: Page, cell: Locator, source: string) {
  const cellHandle = await cell.elementHandle();
  await page.evaluate(
    ([el, src]) => {
      const editors = (window as any).monaco.editor.getEditors();
      for (const editor of editors) {
        if ((el as Element).contains(editor.getDomNode())) {
          editor.getModel()?.setValue(src as string);
          return;
        }
      }
      throw new Error("No Monaco editor found in cell");
    },
    [cellHandle, source] as const
  );
}
