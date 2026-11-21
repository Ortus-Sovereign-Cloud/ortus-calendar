// The calendar's card on the start page (ortus-calendar-today, loaded by
// the core's Today page from /assets/apps/calendar/today.js): what is left
// of today, a call's Join link at hand.
import { LitElement, LocalizeController, call, css, customElement, html, nothing, state } from "ortus-kit";
import type { CalEvent } from "./calendar.js";
import { ready, t } from "./i18n.js";

await ready;

@customElement("ortus-calendar-today")
export class OrtusCalendarToday extends LitElement {
  static override styles = css`
    :host {
      display: block;
    }
    section {
      padding: 20px;
      border-radius: 18px;
      border: 1px solid var(--ortus-line);
      background: var(--ortus-surface);
    }
    h2 {
      margin: 0 0 12px;
      font-size: 17px;
      font-weight: 600;
    }
    ul {
      margin: 0;
      padding: 0;
      list-style: none;
    }
    li {
      display: grid;
      grid-template-columns: 92px 1fr auto;
      gap: 10px;
      align-items: center;
      padding-block: 8px;
      border-block-end: 1px solid var(--ortus-line);
    }
    li:last-child {
      border-block-end: 0;
    }
    time {
      color: var(--ortus-text-2);
      font-size: 13px;
      font-variant-numeric: tabular-nums;
    }
    .what {
      display: flex;
      flex-direction: column;
      min-inline-size: 0;
    }
    .what strong,
    .what span {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .what strong {
      font-size: 14px;
    }
    .what span {
      color: var(--ortus-text-2);
      font-size: 13px;
    }
    .join {
      padding: 4px 12px;
      border-radius: 999px;
      background: var(--ortus-accent);
      color: var(--ortus-bg);
      font-size: 13px;
      font-weight: 600;
      text-decoration: none;
    }
    .quiet {
      margin: 0;
      color: var(--ortus-text-2);
      font-size: 14px;
    }
    footer {
      margin-block-start: 12px;
      font-size: 14px;
    }
    footer a {
      color: var(--ortus-accent);
    }
  `;

  private readonly localize = new LocalizeController(this);
  @state() private events: CalEvent[] = [];
  @state() private loaded = false;

  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    const now = Date.now();
    const end = new Date();
    end.setHours(23, 59, 59, 999);
    const start = new Date();
    start.setHours(0, 0, 0, 0);
    try {
      const { events } = await call<{ events: CalEvent[] }>("GET", `/calendar/events?from=${start.getTime()}&to=${end.getTime()}`);
      // What is left of the day (all-day events stay), the earliest first.
      this.events = events
        .filter((e) => e.allDay || e.end > now)
        .filter((e) => e.answer !== "declined")
        .sort((a, b) => Number(b.allDay) - Number(a.allDay) || a.start - b.start)
        .slice(0, 5);
    } catch {
      this.events = [];
    }
    this.loaded = true;
  }

  private time(e: CalEvent): string {
    if (e.allDay) return t(this.localize, "todayAllDay");
    const f = new Intl.DateTimeFormat(this.localize.lang, { hour: "2-digit", minute: "2-digit" });
    return `${f.format(e.start)}–${f.format(e.end)}`;
  }

  /** A call to join: an Ortus call or a link given as the place. */
  private joinLink(e: CalEvent): string | null {
    const place = e.location.trim();
    if (place.startsWith("/calls")) return place;
    try {
      const url = new URL(place);
      return url.protocol === "https:" ? url.href : null;
    } catch {
      return null;
    }
  }

  override render() {
    if (!this.loaded) return nothing;
    const now = Date.now();
    return html`
      <section aria-labelledby="calendar-today">
        <h2 id="calendar-today">${t(this.localize, "calendar")}</h2>
        ${this.events.length
          ? html`<ul>
              ${this.events.map((e) => {
                const join = this.joinLink(e);
                const soon = !e.allDay && e.start - now < 15 * 60_000;
                return html`<li>
                  <time>${this.time(e)}</time>
                  <span class="what"><strong dir="auto">${e.summary}</strong>${e.location && !join ? html`<span dir="auto">${e.location}</span>` : nothing}</span>
                  ${join && soon ? html`<a class="join" href=${join}>${t(this.localize, "todayJoin")}</a>` : html`<span></span>`}
                </li>`;
              })}
            </ul>`
          : html`<p class="quiet">${t(this.localize, "todayNothing")}</p>`}
        <footer><a href="/calendar?view=day">${t(this.localize, "todayOpen")}</a></footer>
      </section>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    "ortus-calendar-today": OrtusCalendarToday;
  }
}
