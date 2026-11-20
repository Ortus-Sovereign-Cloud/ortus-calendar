// The web calendar (board WebCalendar): the work week with repeats, the
// current time, focus time and an invitation waiting; making an event
// with a colleague and a time that works; the month and a day. Answering
// invitations is tested where mail brings them (the stories).
import {
  expect,
  test,
  type APIRequestContext,
  type Page,
} from "@playwright/test";
import en from "../src/locales/en.js";

test.describe.configure({ mode: "serial" });

const H = 3_600_000;
// Monday 16 Nov 2026, 00:00 UTC (the tests run in UTC).
const MON = Date.UTC(2026, 10, 16);
const NOW = MON + 2 * 24 * H + 11 * H + 15 * 60_000; // Wednesday 11:15

const week = (page: Page) => page.locator("ortus-calendar-week");
const headers = { "X-Requested-With": "ortus" };

async function add(
  request: APIRequestContext,
  body: Record<string, unknown>,
): Promise<{ id: string }> {
  const r = await request.post("/api/v1/calendar/events", {
    headers,
    data: body,
  });
  expect(r.status(), await r.text()).toBe(200);
  return r.json();
}

test.beforeAll(async ({ request }) => {
  // A colleague to invite, and the week of the board.
  await request.post("/api/v1/admin/users", {
    headers,
    data: {
      email: "marek@ministry.example",
      name: "Marek Nowak",
      role: "member",
      groups: [],
      language: "pl",
    },
  });
  const calendars = (
    await (await request.get("/api/v1/calendar/calendars")).json()
  ).calendars;
  const work = calendars[0].id;
  await request.patch(`/api/v1/calendar/calendars/${work}`, {
    headers,
    data: { name: "Work" },
  });
  const regional = (
    await (
      await request.post("/api/v1/calendar/calendars", {
        headers,
        data: { name: "Regional office", color: "orange" },
      })
    ).json()
  ).id;
  await add(request, {
    calendar: work,
    summary: "Standup",
    start: MON + 9 * H + 30 * 60_000,
    end: MON + 9 * H + 45 * 60_000,
    repeat: "weekdays",
  });
  await add(request, {
    calendar: work,
    summary: "Focus: bus plan",
    start: MON + 10 * H + 30 * 60_000,
    end: MON + 12 * H + 30 * 60_000,
    focus: true,
  });
  await add(request, {
    calendar: regional,
    summary: "Regional sync",
    start: MON + 14 * H,
    end: MON + 15 * H,
  });
  await add(request, {
    calendar: work,
    summary: "Week review",
    start: MON + 4 * 24 * H + 15 * H + 30 * 60_000,
    end: MON + 4 * 24 * H + 17 * H,
  });
});

test.beforeEach(async ({ page }) => {
  await page.clock.setFixedTime(NOW);
});

test("the week shows each day's events, repeats and the time now", async ({
  page,
}) => {
  await page.goto("/calendar");
  await expect(
    week(page).getByRole("heading", { level: 1, name: en.calendar }),
  ).toBeVisible();
  await expect(week(page).locator(".range")).toHaveText(/Nov 16\s–\s20, 2026/);
  await expect(week(page).locator(".ev", { hasText: "Standup" })).toHaveCount(
    5,
  );
  await expect(week(page).locator(".ev.focus")).toContainText(
    "Focus: bus plan",
  );
  await expect(week(page).locator(".col.today .now")).toBeVisible();
  // A calendar can be hidden.
  await week(page).getByRole("checkbox", { name: "Regional office" }).uncheck();
  await expect(
    week(page).locator(".ev", { hasText: "Regional sync" }),
  ).toHaveCount(0);
  await week(page).getByRole("checkbox", { name: "Regional office" }).check();
  await expect(
    week(page).locator(".ev", { hasText: "Regional sync" }),
  ).toHaveCount(1);
});

test("a new event with a colleague takes a time that works", async ({
  page,
}) => {
  await page.goto("/calendar");
  await week(page).getByRole("button", { name: en.newEvent }).click();
  const editor = week(page).getByRole("dialog", { name: en.newEvent });
  await editor.getByLabel(en.eventTitle).fill("Plan review");
  await editor.getByLabel(en.date).fill("2026-11-19");
  await editor.getByLabel(en.starts).fill("09:30");
  await editor.getByLabel(en.ends).fill("11:00");
  await editor.getByLabel(en.attendees).fill("marek@ministry.example");
  const times = editor.getByRole("region", { name: en.timesThatWork });
  // 90 minutes: 08:00 fits before the standup (09:30), then 10:00.
  await expect(times.getByRole("button", { name: "08:00" })).toBeVisible();
  await times.getByRole("button", { name: "10:00" }).click();
  await expect(editor.getByLabel(en.starts)).toHaveValue("10:00");
  await editor.getByLabel(en.starts).fill("13:00");
  await editor.getByLabel(en.ends).fill("14:30");
  await editor.getByRole("button", { name: en.save }).click();
  await expect(editor).toBeHidden();
  const review = week(page).locator(".ev", { hasText: "Plan review" });
  await expect(review).toContainText("13:00–14:30");
  await review.click();
  const viewer = week(page).getByRole("dialog", { name: "Plan review" });
  await expect(viewer.getByRole("list", { name: en.attendees })).toContainText(
    "marek@ministry.example",
  );
  await expect(viewer.getByRole("list", { name: en.attendees })).toContainText(
    en.answerWaiting,
  );
  await viewer.getByRole("button", { name: en.close }).click();
  await page.screenshot({ path: "test-results/calendar-week.png" });
});

test("the month and a day", async ({ page }) => {
  await page.goto("/calendar");
  await week(page)
    .getByRole("button", { name: en.viewMonth, exact: true })
    .click();
  await expect(week(page).locator(".range")).toHaveText("November 2026");
  await expect(
    week(page).locator(".month .ev", { hasText: "Week review" }),
  ).toHaveCount(1);
  await week(page).getByRole("button", { name: "November 19" }).click();
  await expect(
    week(page).getByRole("button", { name: en.viewDay, exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(week(page).locator(".ev", { hasText: "Standup" })).toHaveCount(
    1,
  );
});
