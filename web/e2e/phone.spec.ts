// On a phone (390 x 844): nothing wider than the screen, one pane at a
// time. The calendar shows one day (or the month), the month and the
// calendars are behind a button with a way back; People opens a group as
// the whole screen with a way back; the pick page fits.
import { expect, test, type Locator, type Page } from "@playwright/test";
import en from "../src/locales/en.js";

test.use({ viewport: { width: 390, height: 844 } });
test.describe.configure({ mode: "serial" });

const H = 3_600_000;
// Monday 7 Dec 2026, 00:00 UTC: a week no other test uses.
const MON = Date.UTC(2026, 11, 7);
const headers = { "X-Requested-With": "ortus" };

async function fits(page: Page, app: Locator): Promise<void> {
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  const [scroll, client] = await app.evaluate((el) => [el.scrollWidth, el.clientWidth]);
  expect(scroll).toBeLessThanOrEqual(client);
}

/** The element takes (nearly) the app's whole width. */
async function fullWidth(app: Locator, part: Locator): Promise<void> {
  const outer = await app.boundingBox();
  const inner = await part.boundingBox();
  expect(inner && outer && inner.width >= outer.width - 32).toBe(true);
}

test.beforeAll(async ({ request }) => {
  await request.post("/api/v1/admin/users", { headers, data: { email: "iryna@ministry.example", name: "Iryna Bondar", role: "member", groups: ["phone-team"], language: "en" } });
  await request.post("/api/v1/calendar/events", { headers, data: { summary: "Road closures call", start: MON + 9 * H, end: MON + 10 * H, allDay: false, location: "", description: "", attendees: [], repeat: "none" } });
  await request.post("/api/v1/calendar/events", { headers, data: { summary: "Shelter visit", start: MON + 2 * 24 * H + 13 * H, end: MON + 2 * 24 * H + 15 * H, allDay: false, location: "", description: "", attendees: [], repeat: "none" } });
});

test("the calendar on a phone: a day, the side panel behind a button, the month", async ({ page }) => {
  await page.clock.setFixedTime(MON + 8 * H);
  await page.goto(`/calendar?view=week&date=${MON}`);
  const app = page.locator("ortus-calendar-week");
  const grid = app.locator("section.grid");
  // The week is one day at a time; there is no week button.
  await expect(app.getByRole("button", { name: en.viewDay, exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect(app.getByRole("button", { name: en.viewWeek, exact: true })).toHaveCount(0);
  await expect(app.locator(".col")).toHaveCount(1);
  await expect(app.locator(".ev", { hasText: "Road closures call" })).toBeVisible();
  await fits(page, app);
  await fullWidth(app, grid);
  await page.screenshot({ path: "test-results/phone-calendar-day.png" });
  await app.getByRole("button", { name: en.next }).click();
  await app.getByRole("button", { name: en.next }).click();
  await expect(app.locator(".ev", { hasText: "Shelter visit" })).toBeVisible();

  // The month and the calendars: the whole screen, then back.
  await app.getByRole("button", { name: en.calendars }).click();
  await expect(grid).toBeHidden();
  await expect(app.getByRole("checkbox").first()).toBeVisible();
  const back = app.getByRole("button", { name: en.back });
  await expect(back).toBeVisible();
  await fullWidth(app, app.locator("aside"));
  await fits(page, app);
  await page.screenshot({ path: "test-results/phone-calendar-side.png" });
  await back.click();
  await expect(grid).toBeVisible();
  await expect(back).toBeHidden();
  // Choosing a day in the small month goes to it.
  await app.getByRole("button", { name: en.calendars }).click();
  await app.getByRole("button", { name: "December 7" }).click();
  await expect(grid).toBeVisible();
  await expect(app.locator(".ev", { hasText: "Road closures call" })).toBeVisible();

  // An event and the editor fit.
  await app.locator(".ev", { hasText: "Road closures call" }).click();
  const viewer = app.getByRole("dialog", { name: "Road closures call" });
  await expect(viewer).toBeVisible();
  expect((await viewer.boundingBox())!.width).toBeLessThanOrEqual(390);
  await page.screenshot({ path: "test-results/phone-calendar-event.png" });
  await viewer.getByRole("button", { name: en.edit }).click();
  const editor = app.getByRole("dialog", { name: en.editEvent });
  await expect(editor).toBeVisible();
  expect((await editor.boundingBox())!.width).toBeLessThanOrEqual(390);
  await page.screenshot({ path: "test-results/phone-calendar-editor.png" });
  await editor.getByRole("button", { name: en.cancel }).click();

  await app.getByRole("button", { name: en.viewMonth, exact: true }).click();
  await expect(app.locator(".month .ev", { hasText: "Shelter visit" })).toHaveCount(1);
  await fits(page, app);
  await page.screenshot({ path: "test-results/phone-calendar-month.png" });
});

test("People on a phone: sections over the list, a group with a way back", async ({ page }) => {
  await page.goto("/people");
  const app = page.locator("ortus-people-app");
  await expect(app.getByRole("list", { name: en.organisation })).toContainText("Iryna Bondar");
  await fits(page, app);
  await page.screenshot({ path: "test-results/phone-people.png" });
  await app.getByRole("navigation").getByRole("button", { name: /Groups/ }).click();
  await app.getByRole("button", { name: /phone-team/ }).click();
  const members = app.getByRole("list", { name: "phone-team" });
  await expect(members).toContainText("Iryna Bondar");
  await expect(app.getByRole("navigation")).toBeHidden();
  await fullWidth(app, members);
  await fits(page, app);
  await page.screenshot({ path: "test-results/phone-people-group.png" });
  await app.getByRole("button", { name: en.back }).click();
  await expect(app.getByRole("list", { name: en.groupsLists })).toBeVisible();
  await expect(app.getByRole("navigation")).toBeVisible();
  await app.getByRole("button", { name: en.newContact }).click();
  const dialog = app.getByRole("dialog", { name: en.newContact });
  await expect(dialog).toBeVisible();
  expect((await dialog.boundingBox())!.width).toBeLessThanOrEqual(390);
  await page.screenshot({ path: "test-results/phone-people-contact.png" });
});

test("the pick page fits a phone", async ({ browser, request }) => {
  const soon = Math.ceil(Date.now() / H + 96) * H;
  const offer = await (await request.post("/api/v1/calendar/polls", { headers, data: { summary: "Water supply review", minutes: 30, times: [soon, soon + H], people: ["anna@regional.example"] } })).json();
  const visitor = await browser.newContext({ storageState: { cookies: [], origins: [] }, viewport: { width: 390, height: 844 } });
  const page = await visitor.newPage();
  await page.goto(new URL(offer.url).pathname);
  const pick = page.locator("ortus-calendar-public");
  await expect(pick.getByRole("button")).toHaveCount(2);
  await page.screenshot({ path: "test-results/phone-pick.png" });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await visitor.close();
});
