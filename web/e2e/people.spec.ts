// People (board WebContacts): the organisation, a group, and one's own
// contacts made, found, changed and removed.
import { expect, test, type Page } from "@playwright/test";
import en from "../src/locales/en.js";

test.describe.configure({ mode: "serial" });

const people = (page: Page) => page.locator("ortus-people-app");
const headers = { "X-Requested-With": "ortus" };

test.beforeAll(async ({ request }) => {
  for (const [email, name] of [
    ["katarzyna@ministry.example", "Katarzyna Lis"],
    ["sam@ministry.example", "Sam Kim"],
  ]) {
    await request.post("/api/v1/admin/users", { headers, data: { email, name, role: "member", groups: ["flood-team"], language: "en" } });
  }
});

test("the organisation lists colleagues with their groups, and a group", async ({ page }) => {
  await page.goto("/people");
  const list = people(page).getByRole("list", { name: en.organisation });
  await expect(list).toContainText("Katarzyna Lis");
  await expect(list).toContainText("Olena Hrytsenko");
  await expect(list.getByRole("listitem").filter({ hasText: "Sam Kim" })).toContainText("flood-team");
  await expect(list.getByRole("link", { name: "Write to sam@ministry.example" })).toHaveAttribute("href", "/mail/compose?to=sam%40ministry.example");
  await people(page).getByRole("searchbox", { name: en.searchPeople }).fill("katar");
  await expect(list.getByRole("listitem")).toHaveCount(1);
  await people(page).getByRole("searchbox", { name: en.searchPeople }).fill("");
  await people(page).getByRole("navigation").getByRole("button", { name: /Groups/ }).click();
  await people(page).getByRole("button", { name: /flood-team/ }).click();
  await expect(people(page).getByRole("region", { name: "flood-team" })).toContainText("2 members");
  await expect(people(page).getByRole("list", { name: "flood-team" }).getByRole("listitem")).toHaveCount(2);
});

test("contacts are made, found, changed and removed", async ({ page }) => {
  await page.goto("/people?section=contacts");
  await expect(people(page).getByRole("status")).toHaveText(en.peopleEmpty);
  await people(page).getByRole("button", { name: en.newContact }).click();
  const dialog = people(page).getByRole("dialog", { name: en.newContact });
  await dialog.getByLabel(en.contactName).fill("Anna Kowalska");
  await dialog.getByLabel(en.email).fill("anna@regional.example");
  await dialog.getByLabel(en.phone).fill("+00 100 300 117");
  await dialog.getByLabel(en.org).fill("Regional office");
  await dialog.getByLabel(en.jobTitle).fill("Regional coordinator");
  await dialog.getByRole("button", { name: en.save }).click();
  const list = people(page).getByRole("list", { name: en.myContacts });
  await expect(list).toContainText("Regional coordinator");
  await expect(list).toContainText("+00 100 300 117");
  await page.screenshot({ path: "test-results/people.png" });
  await list.getByRole("button", { name: "Edit Anna Kowalska" }).click();
  const edit = people(page).getByRole("dialog", { name: en.editContact });
  await edit.getByLabel(en.phone).fill("+00 100 300 118");
  await edit.getByRole("button", { name: en.save }).click();
  await expect(list).toContainText("+00 100 300 118");
  await list.getByRole("button", { name: "Edit Anna Kowalska" }).click();
  await people(page).getByRole("dialog", { name: en.editContact }).getByRole("button", { name: en.delete }).click();
  await expect(people(page).getByRole("status")).toHaveText(en.peopleEmpty);
});
