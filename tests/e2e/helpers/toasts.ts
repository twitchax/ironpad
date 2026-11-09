import { Page } from "@playwright/test";

/**
 * Start counting toasts titled exactly `title` as they APPEAR, and return a
 * reader for the count.
 *
 * Counting what is on screen ties an assertion to how long each toast lives:
 * under load, the gap between two actions can outlast the first toast, and
 * "two toasts" then reads as one, or none. Counting appearances does not
 * care. The counter lives in the page, so it covers one document.
 */
export async function countToasts(
  page: Page,
  title: string,
): Promise<() => Promise<number>> {
  const key = `__ironpadToastCount:${title}`;
  await page.evaluate(
    ([counter, wanted]) => {
      const w = window as unknown as Record<string, number>;
      if (w[counter] !== undefined) return;
      w[counter] = 0;
      const seen = new WeakSet<Element>();
      const scan = () =>
        document.querySelectorAll(".ironpad-toast-title").forEach((el) => {
          if (!seen.has(el) && el.textContent?.trim() === wanted) {
            seen.add(el);
            w[counter]++;
          }
        });
      new MutationObserver(scan).observe(document.body, {
        childList: true,
        subtree: true,
        characterData: true,
      });
      scan();
    },
    [key, title] as const,
  );
  return () =>
    page.evaluate(
      (counter) => (window as unknown as Record<string, number>)[counter],
      key,
    );
}
