import type { Message } from "ortus-kit";

// The calendar and people apps' texts in English: the keys every other catalog has.
const en = {
  "calendar": "Calendar",
  "calendarEmpty": "Nothing planned yet.",
  "people": "People",
  "peopleEmpty": "No contacts yet.",
};

export type CalendarKey = keyof typeof en;
/** A text, or its plural forms. */
export type CalendarMessage = Message;
export default en;
