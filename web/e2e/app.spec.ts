// The calendar and people apps load in the shell, in every language.
import { expect, test } from "@playwright/test";
import ar from "../src/locales/ar.js";
import de from "../src/locales/de.js";
import en from "../src/locales/en.js";
import es from "../src/locales/es.js";
import fr from "../src/locales/fr.js";
import it from "../src/locales/it.js";
import pl from "../src/locales/pl.js";
import uk from "../src/locales/uk.js";

for (const [code, texts, dir] of [
  ["en", en, "ltr"],
  ["pl", pl, "ltr"],
  ["uk", uk, "ltr"],
  ["de", de, "ltr"],
  ["fr", fr, "ltr"],
  ["es", es, "ltr"],
  ["it", it, "ltr"],
  ["ar", ar, "rtl"],
] as const) {
  test(`calendar and people open in ${code}`, async ({ page }) => {
    const errors: string[] = [];
    page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
    await page.addInitScript(
      (c) => localStorage.setItem("ortus.locale", c),
      code,
    );
    await page.goto("/calendar");
    await expect(
      page
        .locator("ortus-calendar-app")
        .getByRole("heading", { level: 1, name: texts.calendar as string }),
    ).toBeVisible();
    await page.goto("/people");
    await expect(
      page
        .locator("ortus-people-app")
        .getByRole("heading", { level: 1, name: texts.people as string }),
    ).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("dir", dir);
    expect(errors).toEqual([]);
  });
}
