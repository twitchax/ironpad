/**
 * Helpers for driving the Monaco editor from Playwright tests.
 */
import { Page, Locator } from "@playwright/test";

/**
 * Set a cell's Monaco editor content via the Monaco API, as an edit over the
 * whole buffer. Not `setValue`: the bridge ignores flushes (a program write is
 * not typing), so a `setValue` here would never reach the app.
 */
export async function setCellSource(page: Page, cell: Locator, source: string) {
  const cellHandle = await cell.elementHandle();
  await page.evaluate(
    ([el, src]) => {
      const editors = (window as any).monaco.editor.getEditors();
      for (const editor of editors) {
        if ((el as Element).contains(editor.getDomNode())) {
          const model = editor.getModel();
          model?.pushEditOperations(
            [],
            [{ range: model.getFullModelRange(), text: src as string }],
            () => null,
          );
          return;
        }
      }
      throw new Error("No Monaco editor found in cell");
    },
    [cellHandle, source] as const
  );
}
