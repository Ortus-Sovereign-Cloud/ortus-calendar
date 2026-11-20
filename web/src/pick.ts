// The page a link from "Times that work" opens (/calendar/public/pick/…):
// anyone picks one of the offered times, no account needed.
import { ApiError, LitElement, LocalizeController, MARK, call, controls, css, customElement, html, nothing, state, unsafeSVG } from "ortus-kit";
import { t } from "./i18n.js";

interface Offered {
  summary: string;
  organizer: string;
  minutes: number;
  times: number[];
  picked: number | null;
}

function when(ms: number): string {
  return new Intl.DateTimeFormat(document.documentElement.lang || "en", { weekday: "long", day: "numeric", month: "long", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(new Date(ms));
}

@customElement("ortus-calendar-public")
export class OrtusCalendarPublic extends LitElement {
  static override styles = [
    controls,
    css`
      :host {
        display: grid;
        place-items: center;
        min-block-size: 100vh;
        padding: 24px;
        box-sizing: border-box;
        color: var(--ortus-text);
        background: var(--ortus-bg);
      }
      main {
        inline-size: min(520px, 100%);
        display: grid;
        gap: 18px;
        padding: 32px;
        border-radius: 18px;
        border: 1px solid var(--ortus-line);
        background: var(--ortus-surface);
      }
      .mark {
        inline-size: 36px;
        block-size: 36px;
      }
      h1 {
        margin: 0;
        font-size: 24px;
      }
      p {
        margin: 0;
        color: var(--ortus-text-2);
        line-height: 1.5;
      }
      ul {
        margin: 0;
        padding: 0;
        list-style: none;
        display: grid;
        gap: 10px;
      }
      li button {
        inline-size: 100%;
        justify-content: start;
        block-size: 52px;
        font-size: 16px;
      }
      .quiet {
        font-size: 14px;
        color: var(--ortus-text-3);
      }
      .error {
        color: var(--ortus-red);
      }
      .done {
        color: var(--ortus-teal);
        font-weight: 600;
      }
    `,
  ];

  private readonly localize = new LocalizeController(this);
  @state() private offered: Offered | null = null;
  @state() private problem = "";
  @state() private chosen: number | null = null;
  @state() private busy = false;

  private token(): string {
    return location.pathname.split("/").pop() ?? "";
  }

  override connectedCallback(): void {
    super.connectedCallback();
    void call<Offered>("GET", `/calendar/polls/${encodeURIComponent(this.token())}`)
      .then((o) => (this.offered = o))
      .catch(() => (this.problem = t(this.localize, "pickEnded")));
  }

  private async pick(time: number): Promise<void> {
    this.busy = true;
    this.problem = "";
    try {
      await call("POST", `/calendar/polls/${encodeURIComponent(this.token())}/pick`, { time });
      this.chosen = time;
    } catch (e) {
      this.problem = e instanceof ApiError && e.status === 409 ? t(this.localize, "pickTaken") : t(this.localize, "couldNotSave");
      const fresh = await call<Offered>("GET", `/calendar/polls/${encodeURIComponent(this.token())}`).catch(() => null);
      if (fresh) this.offered = fresh;
    } finally {
      this.busy = false;
    }
  }

  override render() {
    const o = this.offered;
    return html`<main>
      <span class="mark">${unsafeSVG(MARK)}</span>
      <h1>${t(this.localize, "pickTitle")}</h1>
      ${o
        ? html`<p>${t(this.localize, "pickLead", { who: o.organizer, what: o.summary, minutes: o.minutes })}</p>
            ${this.chosen !== null
              ? html`<p class="done" role="status">${t(this.localize, "pickDone", { when: when(this.chosen) })}</p>`
              : o.picked !== null
                ? html`<p role="status">${t(this.localize, "pickTaken")}</p>`
                : o.times.length
                  ? html`<ul>
                      ${o.times.map((time) => html`<li><button class="button" ?disabled=${this.busy} @click=${() => this.pick(time)}>${when(time)}</button></li>`)}
                    </ul>`
                  : html`<p role="status">${t(this.localize, "pickNone")}</p>`}`
        : nothing}
      ${this.problem ? html`<p class="error" role="alert">${this.problem}</p>` : nothing}
      <p class="quiet">${t(this.localize, "pickNoAccount")}</p>
    </main>`;
  }
}
