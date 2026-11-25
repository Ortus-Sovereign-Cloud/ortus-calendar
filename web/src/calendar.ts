// The calendar (board WebCalendar): the work week, a day or a month, the
// month at a glance and one's calendars on the side, and the event
// editor with attendees, their free times, repeats and answers.
import {
  ApiError,
  LitElement,
  LocalizeController,
  call,
  controls,
  css,
  customElement,
  html,
  me,
  nothing,
  query,
  repeat,
  state,
  type Me,
  type PropertyValues,
} from "ortus-kit";
import { t } from "./i18n.js";
import type { CalendarKey } from "./locales/en.js";

const DAY = 86_400_000;
const HOUR_PX = 64;
const FIRST_HOUR = 8;

export interface CalendarInfo {
  id: string;
  name: string;
  color: string;
}

export interface Attendee {
  email: string;
  name: string | null;
  answer: string;
}

export interface CalEvent {
  id: string;
  calendar: string;
  uid: string;
  summary: string;
  start: number;
  end: number;
  allDay: boolean;
  location: string;
  description: string;
  organizer: string | null;
  attendees: Attendee[];
  mine: boolean;
  answer: string | null;
  repeat: string;
  focus: boolean;
  private: boolean;
}

type View = "day" | "week" | "month";

const REPEATS = ["none", "daily", "weekdays", "weekly", "monthly"] as const;
const REPEAT_KEYS: Record<(typeof REPEATS)[number], CalendarKey> = {
  none: "repeatNone",
  daily: "repeatDaily",
  weekdays: "repeatWeekdays",
  weekly: "repeatWeekly",
  monthly: "repeatMonthly",
};
const ANSWER_KEYS: Record<string, CalendarKey> = {
  ACCEPTED: "answerAccepted",
  TENTATIVE: "answerTentative",
  DECLINED: "answerDeclined",
  "NEEDS-ACTION": "answerWaiting",
};

function lang(): string {
  return document.documentElement.lang || "en";
}

function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function addDays(ms: number, n: number): number {
  const d = new Date(ms);
  d.setDate(d.getDate() + n);
  return d.getTime();
}

/** Monday of the week of `ms`. */
function monday(ms: number): number {
  const d = new Date(startOfDay(ms));
  return addDays(d.getTime(), -((d.getDay() + 6) % 7));
}

function startOfMonth(ms: number): number {
  const d = new Date(startOfDay(ms));
  d.setDate(1);
  return d.getTime();
}

function addMonths(ms: number, n: number): number {
  const d = new Date(startOfMonth(ms));
  d.setMonth(d.getMonth() + n);
  return d.getTime();
}

function fmt(ms: number, options: Intl.DateTimeFormatOptions): string {
  return new Intl.DateTimeFormat(lang(), options).format(new Date(ms));
}

function hm(ms: number): string {
  return fmt(ms, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
}

/** For <input type=date> and <input type=time>, in local time. */
function dateValue(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function timeValue(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function fromInputs(date: string, time: string): number {
  const [y, m, d] = date.split("-").map(Number);
  const [hh, mm] = (time || "00:00").split(":").map(Number);
  return new Date(y ?? 1970, (m ?? 1) - 1, d ?? 1, hh ?? 0, mm ?? 0).getTime();
}

function nextHalfHour(now: number): number {
  const step = 30 * 60_000;
  return Math.ceil((now + 1) / step) * step;
}

interface Draft {
  id: string | null;
  calendar: string;
  summary: string;
  date: string;
  startTime: string;
  endTime: string;
  allDay: boolean;
  location: string;
  description: string;
  attendees: string;
  repeat: string;
  focus: boolean;
  private: boolean;
}

@customElement("ortus-calendar-week")
export class OrtusCalendarWeek extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: grid;
        grid-template-rows: auto 1fr;
        block-size: var(--ortus-screen, 100vh);
        box-sizing: border-box;
        color: var(--ortus-text);
      }
      /* Previous, Today and next stay together when the header wraps. */
      .steps {
        display: flex;
        gap: 8px;
        align-items: center;
      }
      header.top {
        display: flex;
        align-items: center;
        gap: 12px;
        padding-block: 14px;
        padding-inline: 24px;
        border-block-end: 1px solid var(--ortus-line);
      }
      .segment {
        display: inline-flex;
        gap: 4px;
        padding: 4px;
        border-radius: 12px;
        border: 1px solid var(--ortus-line-strong);
        background: var(--ortus-surface);
      }
      .segment button,
      .segment h1 {
        margin: 0;
        block-size: 32px;
        padding-inline: 14px;
        border: 0;
        border-radius: 8px;
        background: transparent;
        color: var(--ortus-text-3);
        font: inherit;
        font-size: 14px;
        font-weight: 600;
        cursor: pointer;
        display: inline-flex;
        align-items: center;
      }
      .segment h1 {
        font-size: 16px;
        color: var(--ortus-text);
        background: var(--ortus-hover);
        cursor: default;
      }
      .segment button[aria-pressed="true"] {
        background: var(--ortus-hover);
        color: var(--ortus-text);
      }
      .spacer {
        flex: 1;
      }
      .nav {
        inline-size: 40px;
        block-size: 40px;
        padding: 0;
      }
      .range {
        margin: 0;
        min-inline-size: 150px;
        font-size: 17px;
        font-weight: 600;
      }
      kbd {
        margin-inline-start: 8px;
        font: 600 12px var(--ortus-mono, monospace);
        color: var(--ortus-text);
      }
      .body {
        display: grid;
        grid-template-columns: 180px 1fr;
        gap: 18px;
        padding-block: 20px;
        padding-inline: 24px;
        min-block-size: 0;
      }
      aside {
        display: flex;
        flex-direction: column;
        gap: 12px;
      }
      .new {
        block-size: 44px;
        border-radius: 12px;
        border: 0;
        background: var(--ortus-accent);
        color: #0a0b10;
        font: inherit;
        font-size: 15px;
        font-weight: 600;
        cursor: pointer;
      }
      .mini h2,
      .cals h2 {
        margin-block: 18px 12px;
        margin-inline: 0;
        font-size: 15px;
        font-weight: 600;
      }
      .cals h2 {
        font-size: 13px;
        color: var(--ortus-text-3);
        font-weight: 500;
      }
      .mini table {
        inline-size: 100%;
        border-collapse: collapse;
        font-size: 13px;
      }
      .mini th {
        font-weight: 500;
        color: var(--ortus-text-3);
        block-size: 22px;
      }
      .mini td {
        padding: 0;
        text-align: center;
      }
      .mini td button {
        inline-size: 24px;
        block-size: 24px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: transparent;
        color: var(--ortus-text-2);
        font: inherit;
        cursor: pointer;
        margin-block: 3px;
      }
      .mini td button.shown {
        background: var(--ortus-surface-2);
        color: var(--ortus-text);
      }
      .mini td button.today {
        background: var(--ortus-accent);
        color: #0a0b10;
        font-weight: 700;
      }
      .cals label {
        display: flex;
        align-items: center;
        gap: 12px;
        block-size: 44px;
        font-size: 15px;
        cursor: pointer;
      }
      .cals input {
        inline-size: 16px;
        block-size: 16px;
        margin: 0;
      }
      .grid {
        position: relative;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
        overflow: hidden;
        display: grid;
        grid-template-rows: auto 1fr;
        min-block-size: 0;
      }
      .heads,
      .cols {
        display: grid;
        grid-template-columns: 64px repeat(var(--days), 1fr);
      }
      .heads > div {
        block-size: 52px;
        display: flex;
        align-items: center;
        gap: 8px;
        padding-inline: 16px;
        border-block-end: 1px solid var(--ortus-line);
        border-inline-start: 1px solid var(--ortus-line);
        font-size: 13px;
        color: var(--ortus-text-2);
      }
      .heads > div:first-child {
        border-inline-start: 0;
      }
      .heads b {
        font-size: 18px;
        color: var(--ortus-text);
        padding-inline: 4px;
        border-radius: 6px;
      }
      .heads .today b {
        background: var(--ortus-accent);
        color: #0a0b10;
      }
      .alldays {
        display: flex;
        flex-direction: column;
        gap: 2px;
      }
      .scroll {
        overflow-y: auto;
        min-block-size: 0;
      }
      .hours {
        position: relative;
      }
      .hours div {
        block-size: ${HOUR_PX}px;
        box-sizing: border-box;
        padding-inline-start: 18px;
        padding-block-start: 8px;
        font-size: 13px;
        color: var(--ortus-text-3);
        border-block-end: 1px solid var(--ortus-line);
      }
      .col {
        position: relative;
        border-inline-start: 1px solid var(--ortus-line);
        background-image: repeating-linear-gradient(
          to bottom,
          transparent 0 ${HOUR_PX - 1}px,
          var(--ortus-line) ${HOUR_PX - 1}px ${HOUR_PX}px
        );
        block-size: ${HOUR_PX * 24}px;
        cursor: cell;
      }
      .col.today {
        background-color: color-mix(
          in srgb,
          var(--ortus-surface-2) 70%,
          var(--ortus-hover)
        );
      }
      .ev {
        position: absolute;
        inset-inline: 4px;
        box-sizing: border-box;
        padding-block: 6px;
        padding-inline: 10px;
        border-radius: 8px;
        border: 1px solid color-mix(in srgb, var(--c) 30%, transparent);
        border-inline-start: 3px solid var(--c);
        background: color-mix(in srgb, var(--c) 18%, var(--ortus-surface));
        color: var(--ortus-text);
        font: inherit;
        font-size: 13px;
        text-align: start;
        overflow: hidden;
        cursor: pointer;
      }
      .ev strong {
        display: block;
        font-weight: 600;
        color: color-mix(in srgb, var(--c) 35%, var(--ortus-text));
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .ev span {
        color: var(--ortus-text-2);
      }
      .ev.focus {
        background: repeating-linear-gradient(
          135deg,
          color-mix(in srgb, var(--c) 12%, var(--ortus-surface)) 0 8px,
          color-mix(in srgb, var(--c) 4%, var(--ortus-surface)) 8px 16px
        );
      }
      .ev.waiting {
        border: 1px dashed var(--c);
      }
      .ev.short {
        padding-block: 2px;
      }
      .ev.declined {
        opacity: 0.5;
      }
      .ev.declined strong {
        text-decoration: line-through;
      }
      .ev.allday {
        position: static;
        margin-inline: 4px;
        padding-block: 2px;
      }
      .now {
        position: absolute;
        inset-inline: 0;
        block-size: 2px;
        background: var(--ortus-red);
        pointer-events: none;
      }
      .now::before {
        content: "";
        position: absolute;
        inset-inline-start: -5px;
        inset-block-start: -4px;
        inline-size: 10px;
        block-size: 10px;
        border-radius: 50%;
        background: var(--ortus-red);
      }
      .month {
        display: grid;
        grid-template-columns: repeat(7, 1fr);
        grid-auto-rows: 1fr;
        min-block-size: 0;
      }
      .month > div {
        border-inline-start: 1px solid var(--ortus-line);
        border-block-start: 1px solid var(--ortus-line);
        padding: 6px;
        min-block-size: 0;
        overflow: hidden;
        display: flex;
        flex-direction: column;
        gap: 2px;
      }
      .month .n {
        font-size: 13px;
        color: var(--ortus-text-2);
      }
      .month .n.today {
        color: var(--ortus-accent);
        font-weight: 700;
      }
      .month .other .n {
        color: var(--ortus-text-3);
        opacity: 0.6;
      }
      .month .ev {
        position: static;
        padding-block: 1px;
        white-space: nowrap;
      }
      dialog {
        inline-size: min(560px, 92vw);
        max-inline-size: calc(100vw - 16px);
        max-block-size: 90dvh;
        box-sizing: border-box;
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
      .row {
        display: flex;
        gap: 10px;
        flex-wrap: wrap;
        align-items: end;
      }
      .row > label {
        flex: 1;
      }
      label.field {
        display: grid;
        gap: 6px;
        font-size: 13px;
        color: var(--ortus-text-2);
      }
      label.check {
        display: flex;
        align-items: center;
        gap: 8px;
        font-size: 14px;
      }
      label.check input {
        flex: none;
        inline-size: 18px;
        block-size: 18px;
        margin: 0;
      }
      .times {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
      }
      .times button {
        block-size: 30px;
        padding-inline: 10px;
        border-radius: 8px;
        border: 1px solid var(--ortus-teal);
        background: transparent;
        color: var(--ortus-teal);
        font: inherit;
        font-size: 13px;
        cursor: pointer;
      }
      .hint {
        margin: 0;
        font-size: 13px;
        color: var(--ortus-text-3);
      }
      .people {
        margin: 0;
        padding: 0;
        list-style: none;
        display: grid;
        gap: 4px;
        font-size: 14px;
      }
      .people li {
        display: flex;
        justify-content: space-between;
        gap: 12px;
      }
      .people span {
        color: var(--ortus-text-3);
      }
      .actions {
        display: flex;
        gap: 10px;
        justify-content: flex-end;
        flex-wrap: wrap;
      }
      .actions .start {
        margin-inline-end: auto;
      }
      .error {
        margin: 0;
        color: var(--ortus-red);
        font-size: 14px;
      }
      .detail {
        display: grid;
        gap: 8px;
        font-size: 14px;
        color: var(--ortus-text-2);
      }
      .detail p {
        margin: 0;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
      }
      .phone {
        display: none;
      }
      /* A phone: one pane at a time. The grid shows one day (or the
         month); the month and the calendars are behind a button. */
      @media (max-width: 720px) {
        header.top {
          flex-wrap: wrap;
          gap: 8px;
          padding-block: 10px;
          padding-inline: 12px;
        }
        .phone {
          display: inline-flex;
        }
        .segment:has(h1) {
          padding: 0;
          border: 0;
          background: none;
        }
        .segment h1 {
          padding-inline: 0;
          background: none;
          font-size: 18px;
        }
        .quick {
          inline-size: 44px;
          padding: 0;
          font-size: 22px;
        }
        .nav,
        .go-today,
        .segment[role="group"] {
          order: 1;
        }
        .segment button {
          padding-inline: 10px;
        }
        .range {
          order: 2;
          flex-basis: 100%;
          min-inline-size: 0;
          font-size: 16px;
        }
        .segment button,
        .segment h1,
        .nav {
          block-size: 44px;
        }
        .nav {
          inline-size: 44px;
        }
        .button {
          min-block-size: 44px;
        }
        .body {
          grid-template-columns: minmax(0, 1fr);
          padding-block: 12px;
          padding-inline: 12px;
        }
        aside {
          display: none;
          overflow-y: auto;
          min-block-size: 0;
        }
        .body.side aside {
          display: flex;
        }
        .body.side .grid {
          display: none;
        }
        .back {
          align-self: start;
          gap: 6px;
        }
        .mini td button {
          inline-size: 44px;
          block-size: 44px;
          max-inline-size: 100%;
          margin-block: 0;
        }
        .heads,
        .cols {
          grid-template-columns: 52px repeat(var(--days), minmax(0, 1fr));
        }
        .heads > div {
          padding-inline: 8px;
          min-inline-size: 0;
          overflow: hidden;
        }
        .hours div {
          padding-inline-start: 8px;
        }
        .month > div {
          padding: 3px;
          min-inline-size: 0;
        }
        .month .ev {
          inset-inline: 0;
          margin: 0;
          padding-inline: 4px;
          font-size: 11px;
        }
        .times button {
          block-size: 44px;
        }
        select.input {
          min-block-size: 44px;
        }
        form {
          padding: 16px;
        }
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);

  @state() private view: View = "week";
  @state() private anchor = Date.now();
  @state() private calendars: CalendarInfo[] = [];
  @state() private off = new Set<string>();
  @state() private events: CalEvent[] = [];
  @state() private draft: Draft | null = null;
  @state() private opened: CalEvent | null = null;
  @state() private times: number[] = [];
  @state() private error = "";
  @state() private busy = false;
  @state() private now = Date.now();
  /** A phone-sized screen: one day instead of the week, the side panel behind a button. */
  @state() private narrow = false;
  /** On a phone: the side panel (month and calendars) shows instead of the grid. */
  @state() private side = false;
  private readonly phone = window.matchMedia("(max-width: 720px)");
  private account: Me | null = null;
  private timer = 0;
  private freeTimer = 0;

  @query("dialog.editor") private editor!: HTMLDialogElement;
  @query("dialog.viewer") private viewer!: HTMLDialogElement;
  @query(".scroll") private scroller?: HTMLElement;

  override connectedCallback(): void {
    super.connectedCallback();
    const p = new URLSearchParams(location.search);
    const v = p.get("view");
    if (v === "day" || v === "week" || v === "month") this.view = v;
    const at = Number(p.get("date"));
    if (at) this.anchor = at;
    try {
      const hidden = JSON.parse(
        localStorage.getItem("ortus.calendar.hidden") ?? "[]",
      ) as string[];
      this.off = new Set(hidden);
    } catch {
      /* no storage */
    }
    this.narrow = this.phone.matches;
    this.phone.addEventListener("change", this.onPhone);
    void this.load();
    this.timer = window.setInterval(() => (this.now = Date.now()), 60_000);
    window.addEventListener("keydown", this.onKey);
  }

  override disconnectedCallback(): void {
    super.disconnectedCallback();
    window.clearInterval(this.timer);
    window.removeEventListener("keydown", this.onKey);
    this.phone.removeEventListener("change", this.onPhone);
  }

  private readonly onPhone = () => {
    this.narrow = this.phone.matches;
    if (!this.narrow) this.side = false;
    void this.load();
  };

  /** What the grid shows: on a phone the week is one day at a time. */
  private get shownView(): View {
    return this.narrow && this.view === "week" ? "day" : this.view;
  }

  private readonly onKey = (e: KeyboardEvent) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      this.newEvent(nextHalfHour(Date.now()));
    }
  };

  protected override firstUpdated(): void {
    this.scrollToMorning();
  }

  protected override updated(changed: PropertyValues): void {
    if (changed.has("view") || changed.has("narrow") || changed.has("side"))
      this.scrollToMorning();
  }

  private scrollToMorning(): void {
    requestAnimationFrame(() => {
      if (this.scroller) this.scroller.scrollTop = FIRST_HOUR * HOUR_PX;
    });
  }

  private range(): [number, number] {
    if (this.shownView === "day")
      return [startOfDay(this.anchor), addDays(startOfDay(this.anchor), 1)];
    if (this.shownView === "week")
      return [monday(this.anchor), addDays(monday(this.anchor), 5)];
    const first = monday(startOfMonth(this.anchor));
    return [first, addDays(first, 42)];
  }

  private async load(): Promise<void> {
    try {
      this.account ??= await me();
      if (!this.calendars.length)
        this.calendars = (
          await call<{ calendars: CalendarInfo[] }>(
            "GET",
            "/calendar/calendars",
          )
        ).calendars;
      const [from, to] = this.range();
      this.events = (
        await call<{ events: CalEvent[] }>(
          "GET",
          `/calendar/events?from=${from}&to=${to}`,
        )
      ).events;
    } catch (e) {
      this.error = e instanceof ApiError ? e.message : String(e);
    }
  }

  private go(view: View, anchor: number): void {
    this.view = view;
    this.anchor = anchor;
    const url = new URL(location.href);
    url.searchParams.set("view", view);
    url.searchParams.set("date", String(startOfDay(anchor)));
    history.replaceState(history.state, "", url);
    void this.load();
  }

  private step(n: number): void {
    if (this.view === "month") this.go("month", addMonths(this.anchor, n));
    else
      this.go(
        this.view,
        addDays(this.anchor, n * (this.shownView === "week" ? 7 : 1)),
      );
  }

  private toggle(id: string): void {
    const hidden = new Set(this.off);
    if (hidden.has(id)) hidden.delete(id);
    else hidden.add(id);
    this.off = hidden;
    try {
      localStorage.setItem(
        "ortus.calendar.hidden",
        JSON.stringify([...hidden]),
      );
    } catch {
      /* no storage */
    }
  }

  private color(calendar: string): string {
    const name =
      this.calendars.find((c) => c.id === calendar)?.color ?? "accent";
    return name === "violet" ? "#b48cff" : `var(--ortus-${name})`;
  }

  private shown(): CalEvent[] {
    return this.events.filter((e) => !this.off.has(e.calendar));
  }

  // --- the editor ---

  private newEvent(start: number, end = start + 30 * 60_000): void {
    this.draft = {
      id: null,
      calendar:
        this.calendars.find((c) => !this.off.has(c.id))?.id ??
        this.calendars[0]?.id ??
        "",
      summary: "",
      date: dateValue(start),
      startTime: timeValue(start),
      endTime: timeValue(end),
      allDay: false,
      location: "",
      description: "",
      attendees: "",
      repeat: "none",
      focus: false,
      private: false,
    };
    this.times = [];
    this.error = "";
    void this.updateComplete.then(() => {
      this.viewer?.close();
      this.editor.showModal();
      this.editor
        .querySelector<HTMLInputElement>("input[name=summary]")
        ?.focus();
    });
  }

  private edit(e: CalEvent): void {
    const me = this.account?.email.toLowerCase() ?? "";
    this.draft = {
      id: e.id,
      calendar: e.calendar,
      summary: e.summary,
      date: dateValue(e.start),
      startTime: timeValue(e.start),
      endTime: timeValue(e.end),
      allDay: e.allDay,
      location: e.location,
      description: e.description,
      attendees: e.attendees
        .map((a) => a.email)
        .filter((a) => a !== me)
        .join(", "),
      repeat: e.repeat === "other" ? "none" : e.repeat,
      focus: e.focus,
      private: e.private,
    };
    this.error = "";
    this.viewer.close();
    void this.updateComplete.then(() => {
      this.editor.showModal();
      this.findTimes();
    });
  }

  private open(e: CalEvent): void {
    this.opened = e;
    this.error = "";
    void this.updateComplete.then(() => this.viewer.showModal());
  }

  private people(): string[] {
    return (this.draft?.attendees ?? "")
      .split(/[,;\s]+/)
      .map((a) => a.trim().toLowerCase())
      .filter((a) => a.includes("@"));
  }

  private set(field: keyof Draft, value: string | boolean): void {
    if (!this.draft) return;
    this.draft = { ...this.draft, [field]: value };
    if (
      field === "attendees" ||
      field === "date" ||
      field === "startTime" ||
      field === "endTime"
    ) {
      window.clearTimeout(this.freeTimer);
      this.freeTimer = window.setTimeout(() => this.findTimes(), 300);
    }
  }

  /** Times that work, that day, for the attendees here. */
  private findTimes(): void {
    const d = this.draft;
    const people = this.people();
    if (!d || !people.length || d.allDay) {
      this.times = [];
      return;
    }
    const minutes = Math.max(
      15,
      Math.round(
        (fromInputs(d.date, d.endTime) - fromInputs(d.date, d.startTime)) /
          60_000,
      ),
    );
    const from = Math.max(
      fromInputs(d.date, "08:00"),
      nextHalfHour(Date.now()),
    );
    const to = fromInputs(d.date, "18:00");
    if (to <= from) {
      this.times = [];
      return;
    }
    void call<{ times: number[] }>(
      "GET",
      `/calendar/free?people=${encodeURIComponent(people.join(","))}&from=${from}&to=${to}&minutes=${minutes}&limit=6`,
    )
      .then((r) => (this.times = r.times))
      .catch(() => (this.times = []));
  }

  private pick(time: number): void {
    const d = this.draft;
    if (!d) return;
    const length =
      fromInputs(d.date, d.endTime) - fromInputs(d.date, d.startTime);
    this.draft = {
      ...d,
      startTime: timeValue(time),
      endTime: timeValue(time + length),
    };
  }

  private async save(e: Event): Promise<void> {
    e.preventDefault();
    const d = this.draft;
    if (!d) return;
    let start = d.allDay
      ? fromInputs(d.date, "00:00")
      : fromInputs(d.date, d.startTime);
    let end = d.allDay ? addDays(start, 1) : fromInputs(d.date, d.endTime);
    if (end <= start) end = addDays(end, 1);
    if (d.allDay) {
      // All-day events are dates; send midnight UTC of the chosen day.
      const [y, m, day] = d.date.split("-").map(Number);
      start = Date.UTC(y ?? 1970, (m ?? 1) - 1, day ?? 1);
      end = start + DAY;
    }
    const body = {
      calendar: d.calendar || undefined,
      tz: Intl.DateTimeFormat().resolvedOptions().timeZone,
      summary: d.summary,
      start,
      end,
      allDay: d.allDay,
      location: d.location,
      description: d.description,
      attendees: this.people(),
      repeat: d.repeat,
      focus: d.focus,
      private: d.private,
    };
    this.busy = true;
    this.error = "";
    try {
      if (d.id) await call("PUT", `/calendar/events/${d.id}`, body);
      else await call("POST", "/calendar/events", body);
      this.editor.close();
      this.draft = null;
      if (!d.id) this.anchor = start;
      await this.load();
    } catch (err) {
      this.error =
        err instanceof ApiError
          ? err.message
          : t(this.localize, "couldNotSave");
    } finally {
      this.busy = false;
    }
  }

  private async removeEvent(event: CalEvent): Promise<void> {
    this.busy = true;
    try {
      await call("DELETE", `/calendar/events/${event.id}`);
      this.viewer.close();
      this.opened = null;
      await this.load();
    } catch (err) {
      this.error =
        err instanceof ApiError
          ? err.message
          : t(this.localize, "couldNotSave");
    } finally {
      this.busy = false;
    }
  }

  private async answer(event: CalEvent, answer: string): Promise<void> {
    this.busy = true;
    try {
      const done = await call<CalEvent>(
        "POST",
        `/calendar/events/${event.id}/rsvp`,
        { answer },
      );
      this.opened = { ...event, answer: done.answer };
      await this.load();
    } catch (err) {
      this.error =
        err instanceof ApiError
          ? err.message
          : t(this.localize, "couldNotSave");
    } finally {
      this.busy = false;
    }
  }

  // --- drawing ---

  override render() {
    return html`${this.renderTop()}
      <div class="body ${this.side ? "side" : ""}">
        ${this.renderSide()}
        <section class="grid" aria-label=${this.rangeTitle()}>
          ${this.shownView === "month" ? this.renderMonth() : this.renderDays()}
        </section>
      </div>
      ${this.renderEditor()} ${this.renderViewer()}`;
  }

  private rangeTitle(): string {
    const [from, to] = this.range();
    if (this.shownView === "day")
      return fmt(from, {
        weekday: "long",
        day: "numeric",
        month: "long",
        year: "numeric",
      });
    if (this.view === "month")
      return fmt(this.anchor, { month: "long", year: "numeric" });
    const last = addDays(to, -1);
    return new Intl.DateTimeFormat(lang(), {
      day: "numeric",
      month: "short",
      year: "numeric",
    }).formatRange(new Date(from), new Date(last));
  }

  private renderTop() {
    const views: [View, CalendarKey][] = [
      ["day", "viewDay"],
      ...(this.narrow ? [] : ([["week", "viewWeek"]] as [View, CalendarKey][])),
      ["month", "viewMonth"],
    ];
    return html`<header class="top">
      <div class="segment"><h1>${t(this.localize, "calendar")}</h1></div>
      <button
        class="button phone"
        aria-expanded=${this.side ? "true" : "false"}
        @click=${() => (this.side = !this.side)}
      >
        ${t(this.localize, "calendars")}
      </button>
      <span class="spacer"></span>
      <span class="steps">
      <button
        class="button nav"
        aria-label=${t(this.localize, "previous")}
        @click=${() => this.step(-1)}
      >
        <span aria-hidden="true">${document.dir === "rtl" ? "›" : "‹"}</span>
      </button>
      <button class="button go-today" @click=${() => this.go(this.view, Date.now())}>
        ${t(this.localize, "today")}
      </button>
      <button
        class="button nav"
        aria-label=${t(this.localize, "next")}
        @click=${() => this.step(1)}
      >
        <span aria-hidden="true">${document.dir === "rtl" ? "‹" : "›"}</span>
      </button>
      </span>
      <p class="range" aria-live="polite">${this.rangeTitle()}</p>
      <div class="segment" role="group" aria-label=${t(this.localize, "view")}>
        ${views.map(([v, key]) => html`<button aria-pressed=${this.shownView === v ? "true" : "false"} @click=${() => this.go(v, this.anchor)}>${t(this.localize, key)}</button>`)}
      </div>
      <button
        class="button quick"
        aria-label=${this.narrow ? t(this.localize, "quickAdd") : nothing}
        @click=${() => this.newEvent(nextHalfHour(Date.now()))}
      >
        ${this.narrow ? html`<span aria-hidden="true">+</span>` : html`${t(this.localize, "quickAdd")}<kbd>Ctrl K</kbd>`}
      </button>
    </header>`;
  }

  private renderSide() {
    const first = startOfMonth(this.anchor);
    const start = monday(first);
    const month = new Date(first).getMonth();
    const today = startOfDay(this.now);
    const [from, to] = this.range();
    const weeks: number[][] = [];
    for (let w = 0; w < 6; w++) {
      const row: number[] = [];
      for (let d = 0; d < 7; d++) row.push(addDays(start, w * 7 + d));
      if (w === 5 && new Date(row[0] ?? 0).getMonth() !== month) break;
      weeks.push(row);
    }
    const names = [0, 1, 2, 3, 4, 5, 6].map((i) =>
      fmt(addDays(start, i), { weekday: "narrow" }),
    );
    return html`<aside>
      <button class="button phone back" @click=${() => (this.side = false)}>
        <span aria-hidden="true">${document.dir === "rtl" ? "›" : "‹"}</span>
        ${t(this.localize, "back")}
      </button>
      <button
        class="new"
        @click=${() => this.newEvent(nextHalfHour(Date.now()))}
      >
        ${t(this.localize, "newEvent")}
      </button>
      <div class="mini">
        <h2>${fmt(first, { month: "long" })}</h2>
        <table
          role="grid"
          aria-label=${fmt(first, { month: "long", year: "numeric" })}
        >
          <thead>
            <tr>
              ${names.map((n) => html`<th scope="col">${n}</th>`)}
            </tr>
          </thead>
          <tbody>
            ${weeks.map(
              (row) =>
                html`<tr>
                  ${row.map((day) =>
                    new Date(day).getMonth() !== month
                      ? html`<td></td>`
                      : html`<td>
                          <button
                            class="${day === today ? "today" : ""} ${day >= from && day < to && this.shownView !== "month" ? "shown" : ""}"
                            aria-label=${fmt(day, { day: "numeric", month: "long" })}
                            @click=${() => ((this.side = false), this.go(this.view === "month" ? "day" : this.view, day))}
                          >
                            ${new Date(day).getDate()}
                          </button>
                        </td>`,
                  )}
                </tr>`,
            )}
          </tbody>
        </table>
      </div>
      <div class="cals">
        <h2>${t(this.localize, "calendars")}</h2>
        ${repeat(
          this.calendars,
          (c) => c.id,
          (c) =>
            html`<label
              ><input
                type="checkbox"
                .style=${`accent-color: ${this.color(c.id)}`}
                .checked=${!this.off.has(c.id)}
                @change=${() => this.toggle(c.id)}
              /><span>${c.name}</span></label
            >`,
        )}
      </div>
    </aside>`;
  }

  private renderDays() {
    const [from] = this.range();
    const count = this.shownView === "day" ? 1 : 5;
    const days = Array.from({ length: count }, (_, i) => addDays(from, i));
    const today = startOfDay(this.now);
    const events = this.shown();
    return html`<div class="heads" .style=${`--days: ${count}`}>
        <div></div>
        ${days.map(
          (d) =>
            html`<div class=${d === today ? "today" : ""}>
              <span>${fmt(d, { weekday: "short" })}</span
              ><b>${new Date(d).getDate()}</b>
              <span class="alldays"
                >${events.filter((e) => e.allDay && e.start < addDays(d, 1) + DAY / 2 && e.end > d + DAY / 2).map((e) => this.renderEvent(e, null))}</span
              >
            </div>`,
        )}
      </div>
      <div class="scroll">
        <div class="cols" .style=${`--days: ${count}`}>
          <div class="hours">
            ${Array.from({ length: 24 }, (_, h) => html`<div>${hm(addDays(from, 0) + h * 3_600_000)}</div>`)}
          </div>
          ${days.map((d) => {
            const mine = events.filter(
              (e) => !e.allDay && e.start < addDays(d, 1) && e.end > d,
            );
            return html`<div
              class="col ${d === today ? "today" : ""}"
              data-day=${d}
              @click=${(ev: MouseEvent) => this.clickColumn(ev, d)}
            >
              ${mine.map((e) => this.renderEvent(e, d))}
              ${d === today ? html`<div class="now" .style=${`inset-block-start: ${((this.now - d) / 3_600_000) * HOUR_PX}px`}></div>` : nothing}
            </div>`;
          })}
        </div>
      </div>`;
  }

  private clickColumn(ev: MouseEvent, day: number): void {
    if (ev.target !== ev.currentTarget) return;
    const box = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    const hours = (ev.clientY - box.top) / HOUR_PX;
    const start = day + Math.floor(hours * 2) * 30 * 60_000;
    this.newEvent(start, start + 3_600_000);
  }

  private renderEvent(e: CalEvent, day: number | null) {
    const classes = [
      "ev",
      e.end - e.start <= 30 * 60_000 ? "short" : "",
      e.focus ? "focus" : "",
      e.answer === "NEEDS-ACTION" ? "waiting" : "",
      e.answer === "DECLINED" ? "declined" : "",
      day === null ? "allday" : "",
    ].join(" ");
    let style = `--c: ${this.color(e.calendar)};`;
    if (day !== null && this.view !== "month") {
      const top = Math.max(0, (e.start - day) / 3_600_000) * HOUR_PX;
      const bottom = Math.min(24, (e.end - day) / 3_600_000) * HOUR_PX;
      style += `inset-block-start: ${top}px; block-size: ${Math.max(22, bottom - top - 2)}px;`;
    }
    const long =
      e.end - e.start > 45 * 60_000 && day !== null && this.view !== "month";
    return html`<button
      class=${classes}
      .style=${style}
      @click=${(ev: Event) => (ev.stopPropagation(), this.open(e))}
    >
      <strong>${e.summary}</strong
      >${long ? html`<span>${hm(e.start)}–${hm(e.end)}</span>` : nothing}
    </button>`;
  }

  private renderMonth() {
    const [from] = this.range();
    const month = new Date(this.anchor).getMonth();
    const events = this.shown();
    const today = startOfDay(this.now);
    return html`<div
        class="heads"
        .style=${"--days: 7; grid-template-columns: repeat(7, minmax(0, 1fr))"}
      >
        ${[0, 1, 2, 3, 4, 5, 6].map((i) => html`<div>${fmt(addDays(from, i), { weekday: "short" })}</div>`)}
      </div>
      <div class="month">
        ${Array.from({ length: 42 }, (_, i) => {
          const d = addDays(from, i);
          const mine = events
            .filter((e) => e.start < addDays(d, 1) && e.end > d)
            .slice(0, 4);
          return html`<div
            class=${new Date(d).getMonth() !== month ? "other" : ""}
          >
            <span class="n ${d === today ? "today" : ""}"
              >${new Date(d).getDate()}</span
            >
            ${mine.map((e) => this.renderEvent(e, d))}
          </div>`;
        })}
      </div>`;
  }

  private renderEditor() {
    const d = this.draft;
    return html`<dialog
      class="editor"
      aria-label=${d?.id ? t(this.localize, "editEvent") : t(this.localize, "newEvent")}
      @close=${() => (this.draft = null)}
    >
      ${
        d
          ? html`<form @submit=${this.save}>
              <h2>
                ${d.id ? t(this.localize, "editEvent") : t(this.localize, "newEvent")}
              </h2>
              <label class="field"
                >${t(this.localize, "eventTitle")}<input
                  class="input"
                  name="summary"
                  required
                  maxlength="300"
                  .value=${d.summary}
                  @input=${(e: Event) => this.set("summary", (e.target as HTMLInputElement).value)}
              /></label>
              <div class="row">
                <label class="field"
                  >${t(this.localize, "date")}<input
                    class="input"
                    type="date"
                    required
                    .value=${d.date}
                    @input=${(e: Event) => this.set("date", (e.target as HTMLInputElement).value)}
                /></label>
                ${
                  d.allDay
                    ? nothing
                    : html`<label class="field"
                          >${t(this.localize, "starts")}<input
                            class="input"
                            type="time"
                            required
                            .value=${d.startTime}
                            @input=${(e: Event) => this.set("startTime", (e.target as HTMLInputElement).value)}
                        /></label>
                        <label class="field"
                          >${t(this.localize, "ends")}<input
                            class="input"
                            type="time"
                            required
                            .value=${d.endTime}
                            @input=${(e: Event) => this.set("endTime", (e.target as HTMLInputElement).value)}
                        /></label>`
                }
              </div>
              <div class="row">
                <label class="check"
                  ><input
                    type="checkbox"
                    .checked=${d.allDay}
                    @change=${(e: Event) => this.set("allDay", (e.target as HTMLInputElement).checked)}
                  />${t(this.localize, "allDay")}</label
                >
                <label class="check"
                  ><input
                    type="checkbox"
                    .checked=${d.focus}
                    @change=${(e: Event) => this.set("focus", (e.target as HTMLInputElement).checked)}
                  />${t(this.localize, "focusTime")}</label
                >
                <label class="check"
                  ><input
                    type="checkbox"
                    .checked=${d.private}
                    @change=${(e: Event) => this.set("private", (e.target as HTMLInputElement).checked)}
                  />${t(this.localize, "private")}</label
                >
              </div>
              <div class="row">
                <label class="field"
                  >${t(this.localize, "calendarField")}
                  <select
                    class="input"
                    .value=${d.calendar}
                    @change=${(e: Event) => this.set("calendar", (e.target as HTMLSelectElement).value)}
                  >
                    ${this.calendars.map((c) => html`<option value=${c.id} ?selected=${c.id === d.calendar}>${c.name}</option>`)}
                  </select></label
                >
                <label class="field"
                  >${t(this.localize, "repeat")}
                  <select
                    class="input"
                    .value=${d.repeat}
                    @change=${(e: Event) => this.set("repeat", (e.target as HTMLSelectElement).value)}
                  >
                    ${REPEATS.map((r) => html`<option value=${r} ?selected=${r === d.repeat}>${t(this.localize, REPEAT_KEYS[r])}</option>`)}
                  </select></label
                >
              </div>
              <label class="field"
                >${t(this.localize, "attendees")}<input
                  class="input"
                  name="attendees"
                  .value=${d.attendees}
                  placeholder="anna@regional.example, marek@ministry.example"
                  @input=${(e: Event) => this.set("attendees", (e.target as HTMLInputElement).value)}
              /></label>
              ${
                this.people().length && !d.allDay
                  ? html`<section
                      aria-label=${t(this.localize, "timesThatWork")}
                    >
                      <p class="hint">
                        ${this.times.length ? t(this.localize, "timesThatWork") : t(this.localize, "noTimes")}
                      </p>
                      <div class="times">
                        ${this.times.map((time) => html`<button type="button" @click=${() => this.pick(time)}>${hm(time)}</button>`)}
                      </div>
                    </section>`
                  : nothing
              }
              <label class="field"
                >${t(this.localize, "location")}<input
                  class="input"
                  .value=${d.location}
                  maxlength="1000"
                  @input=${(e: Event) => this.set("location", (e.target as HTMLInputElement).value)}
              /></label>
              <label class="field"
                >${t(this.localize, "description")}<textarea
                  class="input"
                  rows="3"
                  .value=${d.description}
                  @input=${(e: Event) => this.set("description", (e.target as HTMLTextAreaElement).value)}
                ></textarea>
              </label>
              ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : nothing}
              <div class="actions">
                <button
                  type="button"
                  class="button"
                  @click=${() => this.editor.close()}
                >
                  ${t(this.localize, "cancel")}
                </button>
                <button class="button primary" ?disabled=${this.busy}>
                  ${t(this.localize, "save")}
                </button>
              </div>
            </form>`
          : nothing
      }
    </dialog>`;
  }

  private renderViewer() {
    const e = this.opened;
    const answers: [string, CalendarKey][] = [
      ["accepted", "accept"],
      ["tentative", "maybe"],
      ["declined", "decline"],
    ];
    return html`<dialog
      class="viewer"
      aria-label=${e?.summary ?? ""}
      @close=${() => (this.opened = null)}
    >
      ${
        e
          ? html`<form method="dialog">
              <h2>${e.summary}</h2>
              <div class="detail">
                <p>${e.allDay ? fmt(e.start, { weekday: "long", day: "numeric", month: "long" }) : `${fmt(e.start, { weekday: "long", day: "numeric", month: "long" })} · ${hm(e.start)}–${hm(e.end)}`}</p>
                ${e.location ? html`<p>${e.location}</p>` : nothing}
                ${e.description ? html`<p>${e.description}</p>` : nothing}
                ${e.organizer && !e.mine ? html`<p>${t(this.localize, "invitedBy", { who: e.organizer })}</p>` : nothing}
              </div>
              ${
                e.attendees.length
                  ? html`<ul
                      class="people"
                      aria-label=${t(this.localize, "attendees")}
                    >
                      ${e.attendees.map((a) => html`<li>${a.name || a.email}<span>${t(this.localize, ANSWER_KEYS[a.answer] ?? "answerWaiting")}</span></li>`)}
                    </ul>`
                  : nothing
              }
              ${this.error ? html`<p class="error" role="alert">${this.error}</p>` : nothing}
              <div class="actions">
                ${
                  e.mine
                    ? html`<button
                          type="button"
                          class="button start"
                          ?disabled=${this.busy}
                          @click=${() => this.removeEvent(e)}
                        >
                          ${t(this.localize, "delete")}
                        </button>
                        <button
                          type="button"
                          class="button primary"
                          @click=${() => this.edit(e)}
                        >
                          ${t(this.localize, "edit")}
                        </button>`
                    : html`<span class="start hint"
                          >${t(this.localize, "yourAnswer")}:
                          ${t(this.localize, ANSWER_KEYS[e.answer ?? "NEEDS-ACTION"] ?? "answerWaiting")}</span
                        >
                        ${answers.map(([a, key]) => html`<button type="button" class="button ${e.answer === a.toUpperCase() ? "primary" : ""}" ?disabled=${this.busy} @click=${() => this.answer(e, a)}>${t(this.localize, key)}</button>`)}`
                }
                <button class="button">${t(this.localize, "close")}</button>
              </div>
            </form>`
          : nothing
      }
    </dialog>`;
  }
}
