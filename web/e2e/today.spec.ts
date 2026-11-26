// The calendar card of the start page (ortus-calendar-today, from
// today.js): what is left of today, a call about to start with Join.
import { expect, test } from "@playwright/test";
import en from "../src/locales/en.js";

test("the start page's calendar card shows what is left of today", async ({ page }) => {
  // Noon of today, on the page's clock too: an event five minutes ahead is
  // still today when the test runs just before midnight.
  const noon = new Date();
  noon.setHours(12, 0, 0, 0);
  await page.clock.setFixedTime(noon);
  const start = noon.getTime() + 5 * 60_000;
  const summary = `Flood briefing ${Date.now()}`;
  const made = await page.request.post("/api/v1/calendar/events", {
    headers: { "X-Requested-With": "ortus" },
    data: { summary, start, end: start + 30 * 60_000, allDay: false, location: "/calls?room=flood-response", description: "", attendees: [], repeat: "none" },
  });
  expect(made.status(), await made.text()).toBe(200);
  await page.goto("/calendar");
  await page.evaluate(async () => {
    const card = "/assets/apps/calendar/today.js";
    await import(/* @vite-ignore */ card);
    document.body.append(document.createElement("ortus-calendar-today"));
  });
  const card = page.locator("ortus-calendar-today").getByRole("region", { name: en.calendar });
  const row = card.getByRole("listitem").filter({ hasText: summary });
  await expect(row).toBeVisible();
  await expect(row.getByRole("link", { name: en.todayJoin })).toHaveAttribute("href", "/calls?room=flood-response");
  await expect(card.getByRole("link", { name: en.todayOpen })).toHaveAttribute("href", "/calendar?view=day");
});
