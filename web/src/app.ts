// The calendar and people apps' client: the elements the shell shows at
// /calendar and /people.
import {
  LitElement,
  LocalizeController,
  controls,
  css,
  customElement,
  html,
} from "ortus-kit";
import "./calendar.js";
import "./pick.js";
import { ready, t } from "./i18n.js";

await ready;

const page = css`
  :host {
    display: block;
    padding: 32px 24px;
  }
  h1 {
    margin-block: 0 24px;
    margin-inline: 0;
    font-size: 28px;
    font-weight: 600;
  }
  .empty {
    padding: 48px 24px;
    border-radius: 18px;
    border: 1px dashed var(--ortus-line-strong);
    text-align: center;
  }
  .empty p {
    margin: 0;
    color: var(--ortus-text-2);
  }
`;

@customElement("ortus-calendar-app")
export class OrtusCalendarApp extends LitElement {
  static override styles = css`
    :host {
      display: block;
    }
  `;

  override render() {
    return html`<ortus-calendar-week></ortus-calendar-week>`;
  }
}

@customElement("ortus-people-app")
export class OrtusPeopleApp extends LitElement {
  static override styles = [controls, page];
  private readonly localize = new LocalizeController(this);

  override render() {
    return html`<h1>${t(this.localize, "people")}</h1>
      <section class="empty" role="status">
        <p>${t(this.localize, "peopleEmpty")}</p>
      </section>`;
  }
}
