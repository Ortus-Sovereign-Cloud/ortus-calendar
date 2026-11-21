// People (board WebContacts): the organisation's directory, its groups,
// and one's own contacts, with search; contacts are made and changed here
// (and sync to phones over CardDAV).
import { ApiError, LitElement, LocalizeController, call, controls, css, customElement, html, nothing, query, repeat, state } from "ortus-kit";
import { t } from "./i18n.js";

interface Colleague {
  name: string;
  email: string;
  groups: string[];
}

interface GroupInfo {
  id: string;
  name: string;
  members: number;
}

interface Contact {
  id: string;
  name: string;
  emails: string[];
  phones: string[];
  org: string;
  title: string;
  note: string;
}

type Section = "organisation" | "groups" | "contacts";

interface Draft {
  id: string | null;
  name: string;
  email: string;
  phone: string;
  org: string;
  title: string;
  note: string;
}

function initials(name: string): string {
  const words = name.replace(/[^\p{L}\p{N} ]/gu, " ").split(/\s+/).filter(Boolean);
  return ((words[0]?.[0] ?? "?") + (words.length > 1 ? (words[words.length - 1]?.[0] ?? "") : "")).toUpperCase();
}

function tone(text: string): string {
  let h = 0;
  for (const c of text) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return `tone${h % 6}`;
}

@customElement("ortus-people-app")
export class OrtusPeopleApp extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: grid;
        grid-template-rows: auto 1fr;
        min-block-size: 100vh;
        color: var(--ortus-text);
      }
      header {
        display: flex;
        align-items: center;
        gap: 16px;
        padding-block: 14px;
        padding-inline: 24px;
        border-block-end: 1px solid var(--ortus-line);
      }
      h1 {
        margin: 0;
        font-size: 16px;
        font-weight: 600;
      }
      .search {
        inline-size: min(554px, 50vw);
      }
      .spacer {
        flex: 1;
      }
      .body {
        display: grid;
        grid-template-columns: 196px 1fr 256px;
        gap: 28px;
        padding-block: 20px;
        padding-inline: 12px 24px;
        align-items: start;
      }
      nav {
        display: grid;
        gap: 4px;
      }
      nav button {
        display: flex;
        align-items: center;
        justify-content: space-between;
        block-size: 40px;
        padding-inline: 12px;
        border-radius: 10px;
        border: 1px solid transparent;
        background: transparent;
        color: var(--ortus-text);
        font: inherit;
        font-size: 15px;
        cursor: pointer;
        text-align: start;
      }
      nav button[aria-current="page"] {
        background: var(--ortus-hover);
        border-color: var(--ortus-line-strong);
        font-weight: 600;
      }
      nav small {
        font-size: 13px;
        color: var(--ortus-text-3);
      }
      h2 {
        margin-block: 4px 20px;
        margin-inline: 0;
        font-size: 22px;
      }
      .list {
        margin: 0;
        padding: 0;
        list-style: none;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
        overflow: hidden;
      }
      .list li {
        display: grid;
        grid-template-columns: minmax(0, 2fr) minmax(0, 1.4fr) minmax(0, 1fr) auto;
        align-items: center;
        gap: 16px;
        min-block-size: 64px;
        padding-inline: 20px;
        border-block-start: 1px solid var(--ortus-line);
      }
      .list li:first-child {
        border-block-start: 0;
      }
      .who {
        display: flex;
        align-items: center;
        gap: 14px;
        min-inline-size: 0;
        background: none;
        border: 0;
        color: inherit;
        font: inherit;
        text-align: start;
        padding: 0;
      }
      button.who {
        cursor: pointer;
      }
      .who strong {
        display: block;
        font-size: 15px;
        font-weight: 600;
      }
      .who small,
      .muted {
        font-size: 13px;
        color: var(--ortus-text-3);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .avatar {
        flex: none;
        display: grid;
        place-items: center;
        inline-size: 40px;
        block-size: 40px;
        border-radius: 12px;
        font-size: 14px;
        font-weight: 700;
        color: #0a0b10;
      }
      .tone0 {
        background: #a5b4ff;
      }
      .tone1 {
        background: #6ee0d0;
      }
      .tone2 {
        background: #f9b77a;
      }
      .tone3 {
        background: #d4f27a;
      }
      .tone4 {
        background: #eceef3;
      }
      .tone5 {
        background: #ffa39a;
      }
      .cell {
        font-size: 15px;
        color: var(--ortus-text-2);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .actions {
        display: flex;
        gap: 6px;
      }
      .small {
        block-size: 32px;
        padding-inline: 12px;
        font-size: 13px;
      }
      .empty {
        padding: 48px 24px;
        border-radius: 18px;
        border: 1px dashed var(--ortus-line-strong);
        text-align: center;
        color: var(--ortus-text-2);
      }
      .card {
        padding: 20px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
        display: grid;
        gap: 10px;
      }
      .card p {
        margin: 0;
        font-size: 14px;
        color: var(--ortus-text-2);
      }
      .card .label {
        font-size: 13px;
        color: var(--ortus-text-3);
      }
      .card strong {
        font-size: 17px;
        overflow-wrap: anywhere;
      }
      dialog {
        inline-size: min(520px, 92vw);
        padding: 0;
        border-radius: 18px;
        border: 1px solid var(--ortus-line-strong);
        background: var(--ortus-surface);
        color: var(--ortus-text);
      }
      dialog::backdrop {
        background: rgb(0 0 0 / 0.55);
      }
      form {
        display: grid;
        gap: 14px;
        padding: 24px;
      }
      form h2 {
        margin: 0;
        font-size: 20px;
      }
      label {
        display: grid;
        gap: 6px;
        font-size: 13px;
        color: var(--ortus-text-2);
      }
      .row {
        display: flex;
        gap: 10px;
        justify-content: flex-end;
      }
      .row .start {
        margin-inline-end: auto;
      }
      .error {
        margin: 0;
        color: var(--ortus-red);
        font-size: 14px;
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @state() private section: Section = "organisation";
  @state() private q = "";
  @state() private colleagues: Colleague[] = [];
  @state() private total = 0;
  @state() private groupList: GroupInfo[] = [];
  @state() private group: GroupInfo | null = null;
  @state() private contactList: Contact[] = [];
  @state() private contactCount = 0;
  @state() private draft: Draft | null = null;
  @state() private error = "";
  @state() private busy = false;
  private searchTimer = 0;
  @query("dialog") private dialog!: HTMLDialogElement;

  override connectedCallback(): void {
    super.connectedCallback();
    const s = new URLSearchParams(location.search).get("section");
    if (s === "groups" || s === "contacts") this.section = s;
    void this.load();
  }

  private async load(): Promise<void> {
    try {
      const q = encodeURIComponent(this.q.trim());
      const [dir, groups, mine] = await Promise.all([
        call<{ people: Colleague[]; total: number }>("GET", `/people/directory?q=${q}${this.group ? `&group=${encodeURIComponent(this.group.id)}` : ""}`),
        call<{ groups: GroupInfo[] }>("GET", "/people/groups"),
        call<{ contacts: Contact[]; total: number }>("GET", `/people/contacts?q=${q}`),
      ]);
      this.colleagues = dir.people;
      this.total = dir.total;
      this.groupList = groups.groups;
      this.contactList = mine.contacts;
      this.contactCount = mine.total;
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
    }
  }

  private show(section: Section): void {
    this.section = section;
    this.group = null;
    const url = new URL(location.href);
    url.searchParams.set("section", section);
    history.replaceState(history.state, "", url);
    void this.load();
  }

  private search(value: string): void {
    this.q = value;
    window.clearTimeout(this.searchTimer);
    this.searchTimer = window.setTimeout(() => void this.load(), 200);
  }

  private openGroup(g: GroupInfo): void {
    this.group = g;
    void this.load();
  }

  private edit(c: Contact | null): void {
    this.error = "";
    this.draft = c ? { id: c.id, name: c.name, email: c.emails[0] ?? "", phone: c.phones[0] ?? "", org: c.org, title: c.title, note: c.note } : { id: null, name: "", email: "", phone: "", org: "", title: "", note: "" };
    void this.updateComplete.then(() => {
      this.dialog.showModal();
      this.dialog.querySelector<HTMLInputElement>("input")?.focus();
    });
  }

  private set(field: keyof Draft, value: string): void {
    if (this.draft) this.draft = { ...this.draft, [field]: value };
  }

  private async save(e: Event): Promise<void> {
    e.preventDefault();
    const d = this.draft;
    if (!d) return;
    this.busy = true;
    this.error = "";
    try {
      const body = { name: d.name, email: d.email, phone: d.phone, org: d.org, title: d.title, note: d.note };
      if (d.id) await call("PUT", `/people/contacts/${encodeURIComponent(d.id)}`, body);
      else await call("POST", "/people/contacts", body);
      this.dialog.close();
      if (this.section !== "contacts") this.show("contacts");
      else await this.load();
    } catch (err) {
      this.error = err instanceof ApiError ? err.message : t(this.localize, "couldNotSave");
    } finally {
      this.busy = false;
    }
  }

  private async removeContact(): Promise<void> {
    const d = this.draft;
    if (!d?.id) return;
    this.busy = true;
    try {
      await call("DELETE", `/people/contacts/${encodeURIComponent(d.id)}`);
      this.dialog.close();
      await this.load();
    } catch (err) {
      this.error = err instanceof ApiError ? err.message : t(this.localize, "couldNotSave");
    } finally {
      this.busy = false;
    }
  }

  private mailTo(email: string) {
    return html`<a class="button small" href="/mail/compose?to=${encodeURIComponent(email)}" aria-label=${t(this.localize, "mailTo", { who: email })}>${t(this.localize, "mail")}</a>`;
  }

  override render() {
    const sections: [Section, string, number][] = [
      ["organisation", t(this.localize, "organisation"), this.total],
      ["groups", t(this.localize, "groupsLists"), this.groupList.length],
      ["contacts", t(this.localize, "myContacts"), this.contactCount],
    ];
    return html`<header>
        <h1>${t(this.localize, "people")}</h1>
        <input class="input search" type="search" .value=${this.q} placeholder=${t(this.localize, "searchPeople")} aria-label=${t(this.localize, "searchPeople")} @input=${(e: Event) => this.search((e.target as HTMLInputElement).value)} />
        <span class="spacer"></span>
        <button class="button primary" @click=${() => this.edit(null)}>${t(this.localize, "newContact")}</button>
      </header>
      <div class="body">
        <nav aria-label=${t(this.localize, "people")}>
          ${sections.map(([id, name, count]) => html`<button aria-current=${this.section === id ? "page" : "false"} @click=${() => this.show(id)}><span>${name}</span><small>${count}</small></button>`)}
        </nav>
        <main>${this.section === "contacts" ? this.renderContacts() : this.section === "groups" && !this.group ? this.renderGroups() : this.renderDirectory()}</main>
        <aside>${this.group ? this.renderGroupCard(this.group) : nothing}</aside>
      </div>
      ${this.renderDialog()}`;
  }

  private renderDirectory() {
    return html`<h2>${this.group ? this.group.name : t(this.localize, "organisation")}</h2>
      ${this.colleagues.length
        ? html`<ul class="list" aria-label=${this.group ? this.group.name : t(this.localize, "organisation")}>
            ${repeat(
              this.colleagues,
              (c) => c.email,
              (c) => html`<li>
                <span class="who"><ortus-avatar class="avatar ${tone(c.name || c.email)}" aria-hidden="true" .name=${c.name || c.email} .email=${c.email ?? ""}></ortus-avatar><span><strong dir="auto">${c.name}</strong><small>${c.email}</small></span></span>
                <span class="cell">${c.groups.join(", ")}</span>
                <span></span>
                <span class="actions">${this.mailTo(c.email)}</span>
              </li>`,
            )}
          </ul>`
        : html`<p class="empty" role="status">${t(this.localize, "noPeople")}</p>`}`;
  }

  private renderGroups() {
    return html`<h2>${t(this.localize, "groupsLists")}</h2>
      ${this.groupList.length
        ? html`<ul class="list" aria-label=${t(this.localize, "groupsLists")}>
            ${this.groupList.map(
              (g) => html`<li>
                <button class="who" @click=${() => this.openGroup(g)}><span class="avatar ${tone(g.name)}" aria-hidden="true">${initials(g.name)}</span><span><strong dir="auto">${g.name}</strong><small>${t(this.localize, "members", { count: g.members })}</small></span></button>
                <span></span><span></span><span></span>
              </li>`,
            )}
          </ul>`
        : html`<p class="empty" role="status">${t(this.localize, "noGroups")}</p>`}`;
  }

  private renderGroupCard(g: GroupInfo) {
    return html`<section class="card" aria-label=${g.name}>
      <span class="label">${t(this.localize, "group")}</span>
      <strong dir="auto">${g.name}</strong>
      <p>${t(this.localize, "members", { count: g.members })}</p>
    </section>`;
  }

  private renderContacts() {
    return html`<h2>${t(this.localize, "myContacts")}</h2>
      ${this.contactList.length
        ? html`<ul class="list" aria-label=${t(this.localize, "myContacts")}>
            ${repeat(
              this.contactList,
              (c) => c.id,
              (c) => html`<li>
                <button class="who" @click=${() => this.edit(c)} aria-label=${t(this.localize, "editContactOf", { who: c.name })}>
                  <ortus-avatar class="avatar ${tone(c.name)}" aria-hidden="true" .name=${c.name} .email=${c.emails?.[0] ?? ""}></ortus-avatar><span><strong dir="auto">${c.name}</strong><small dir="auto">${c.title || c.emails[0] || ""}</small></span>
                </button>
                <span class="cell" dir="auto">${c.org}</span>
                <span class="muted" dir="ltr">${c.phones[0] ?? ""}</span>
                <span class="actions">${c.emails[0] ? this.mailTo(c.emails[0]) : nothing}</span>
              </li>`,
            )}
          </ul>`
        : html`<p class="empty" role="status">${this.q ? t(this.localize, "noPeople") : t(this.localize, "peopleEmpty")}</p>`}`;
  }

  private renderDialog() {
    const d = this.draft;
    const field = (key: keyof Draft, label: string, type = "text") =>
      html`<label>${label}<input class="input" type=${type} .value=${d?.[key] ?? ""} ?required=${key === "name"} @input=${(e: Event) => this.set(key, (e.target as HTMLInputElement).value)} /></label>`;
    return html`<dialog aria-label=${d?.id ? t(this.localize, "editContact") : t(this.localize, "newContact")} @close=${() => (this.draft = null)}>
      ${d
        ? html`<form @submit=${this.save}>
            <h2>${d.id ? t(this.localize, "editContact") : t(this.localize, "newContact")}</h2>
            ${field("name", t(this.localize, "contactName"))} ${field("email", t(this.localize, "email"), "email")} ${field("phone", t(this.localize, "phone"), "tel")} ${field("org", t(this.localize, "org"))} ${field("title", t(this.localize, "jobTitle"))}
            <label>${t(this.localize, "note")}<textarea class="input" rows="3" .value=${d.note} @input=${(e: Event) => this.set("note", (e.target as HTMLTextAreaElement).value)}></textarea></label>
            ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : nothing}
            <div class="row">
              ${d.id ? html`<button type="button" class="button start" ?disabled=${this.busy} @click=${this.removeContact}>${t(this.localize, "delete")}</button>` : nothing}
              <button type="button" class="button" @click=${() => this.dialog.close()}>${t(this.localize, "cancel")}</button>
              <button class="button primary" ?disabled=${this.busy}>${t(this.localize, "save")}</button>
            </div>
          </form>`
        : nothing}
    </dialog>`;
  }
}
