// The calendar and people apps' client: the elements the shell shows at
// /calendar and /people, and the calendar's public page.
import { LitElement, css, customElement, html } from "ortus-kit";
import "./calendar.js";
import "./people.js";
import "./pick.js";
import { ready } from "./i18n.js";

await ready;

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
