#!/usr/bin/env bash
# A real CalDAV and CardDAV client (vdirsyncer) against the dev program:
# discovery from the well-known names, sync both ways, changes and
# deletions on either side, and a wrong password refused.
#
#   VDIRSYNCER=<path> ci/dav-client-test.sh      (default: vdirsyncer on PATH)
set -euo pipefail
cd "$(dirname "$0")/.."
vdirsyncer="${VDIRSYNCER:-vdirsyncer}"
if ! command -v "$vdirsyncer" >/dev/null; then
  if [ -n "${ORTUS_REQUIRE_DAV_CLIENT:-}" ]; then echo "dav-client: vdirsyncer is missing" >&2; exit 1; fi
  echo "dav-client: skipped (no vdirsyncer; pip install vdirsyncer)"
  exit 0
fi
work=$(mktemp -d)
web=$((20000 + RANDOM % 20000)); dav=$((web + 1))
cleanup() { kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true; rm -rf "$work"; }
fail() { echo "dav-client: $*" >&2; sed -n '1,200p' "$work/server.log" >&2 || true; exit 1; }
ORTUS_ROLES=web,worker ORTUS_WEB_PORT=$web ORTUS_DAV_LISTEN=127.0.0.1:$dav ORTUS_DATA_DIR="$work/data" \
  ORTUS_SETUP_TOKEN=dav-client-setup-token ORTUS_PUBLIC_URL=http://localhost:$web target/debug/examples/dev >"$work/server.log" 2>&1 &
server=$!
trap cleanup EXIT
for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:$web/api/v1/health" >/dev/null && break; sleep 0.2; done
curl -sf -c "$work/cookies" -H 'Content-Type: application/json' -H 'X-Requested-With: ortus' \
  -d '{"token":"dav-client-setup-token","name":"Olena Hrytsenko","email":"olena@ministry.example"}' "http://127.0.0.1:$web/api/v1/setup" >/dev/null || fail "setup"
password=$(curl -sf -b "$work/cookies" -H 'Content-Type: application/json' -H 'X-Requested-With: ortus' \
  -d '{"name":"vdirsyncer"}' "http://127.0.0.1:$web/api/v1/me/app-passwords" | python3 -c 'import json,sys; print(json.load(sys.stdin)["password"])')
user=olena@ministry.example
dav_url=http://localhost:$dav

# A wrong password is refused, with the challenge apps need.
code=$(curl -s -o /dev/null -w '%{http_code}' -X PROPFIND -u "$user:aaaa-bbbb-cccc-dddd" "$dav_url/dav/calendar/")
[ "$code" = 401 ] || fail "a wrong password got $code"
# The well-known names lead to the right place.
curl -s -i "$dav_url/.well-known/carddav" | grep -qi '^location: /dav/people/' || fail "well-known carddav"

mkdir -p "$work/cal" "$work/card" "$work/status"
cat >"$work/config" <<EOF
[general]
status_path = "$work/status/"
[pair cal]
a = "cal_local"
b = "cal_remote"
collections = ["from b"]
conflict_resolution = "b wins"
[storage cal_local]
type = "filesystem"
path = "$work/cal/"
fileext = ".ics"
[storage cal_remote]
type = "caldav"
url = "$dav_url/.well-known/caldav"
username = "$user"
password = "$password"
[pair card]
a = "card_local"
b = "card_remote"
collections = ["from b"]
conflict_resolution = "b wins"
[storage card_local]
type = "filesystem"
path = "$work/card/"
fileext = ".vcf"
[storage card_remote]
type = "carddav"
url = "$dav_url/.well-known/carddav"
username = "$user"
password = "$password"
EOF
sync() { "$vdirsyncer" -c "$work/config" sync >>"$work/sync.log" 2>&1 || { cat "$work/sync.log" >&2; fail "sync"; }; }
# discover asks before making local folders: yes to each (yes itself ends
# on a closed pipe, so the result is read from the log).
{ yes 2>/dev/null || true; } | "$vdirsyncer" -c "$work/config" discover >"$work/discover.log" 2>&1 || true
[ "$(grep -c "Saved for" "$work/discover.log")" = 2 ] || { cat "$work/discover.log" >&2; fail "discover"; }
sync
calendar=$(ls "$work/cal")
book=$(ls "$work/card" | grep -v '^directory$')
[ -n "$calendar" ] && [ -n "$book" ] || fail "no collections found"
ls "$work/card/directory" | grep -q '\.vcf$' || fail "the directory did not come"

# Up: an event and a contact made in the app reach the server.
cat >"$work/cal/$calendar/review.ics" <<'EOF'
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//vdirsyncer test//EN
BEGIN:VEVENT
UID:review@ministry.example
DTSTAMP:20261120T090000Z
DTSTART:20261126T140000Z
DTEND:20261126T143000Z
SUMMARY:Shelter agreement review
END:VEVENT
END:VCALENDAR
EOF
cat >"$work/card/$book/anna.vcf" <<'EOF'
BEGIN:VCARD
VERSION:3.0
UID:anna-kowalska
FN:Anna Kowalska
EMAIL:anna@regional.example
END:VCARD
EOF
cat >"$work/card/$book/daniel.vcf" <<'EOF'
BEGIN:VCARD
VERSION:4.0
UID:daniel-price
FN:Daniel Price
EMAIL:daniel@hydro.example
END:VCARD
EOF
sync
# The app names what it uploads; find each by its UID.
href_of() { # collection-url uid
  local href
  for href in $(curl -sf -u "$user:$password" -X PROPFIND -H 'Depth: 1' "$1" | grep -o '<D:href>[^<]*</D:href>' | sed 's/<[^>]*>//g'); do
    case "$href" in */) continue;; esac
    if curl -sf -u "$user:$password" "$dav_url$href" | grep -q "UID:$2"; then echo "$href"; return 0; fi
  done
  return 1
}
review=$(href_of "$dav_url/dav/calendar/home/$calendar/" review@ministry.example) || fail "the event did not go up"
anna=$(href_of "$dav_url/dav/people/home/$book/" anna-kowalska) || fail "the contact did not go up"
curl -sf -u "$user:$password" "$dav_url$anna" | grep -q 'Anna Kowalska' || fail "the contact came up changed"

# Down: a change on the server reaches the app; so does a new event.
etag=$(curl -s -i -u "$user:$password" "$dav_url$review" | tr -d '\r' | sed -n 's/^[Ee][Tt][Aa][Gg]: //p')
sed 's/Shelter agreement review/Shelter agreement review with legal/' "$work/cal/$calendar/review.ics" | \
  curl -sf -u "$user:$password" -X PUT -H "If-Match: $etag" -H 'Content-Type: text/calendar' --data-binary @- "$dav_url$review" >/dev/null || fail "server-side change"
code=$(sed 's/review/stale/' "$work/cal/$calendar/review.ics" | curl -s -o /dev/null -w '%{http_code}' -u "$user:$password" -X PUT -H "If-Match: $etag" --data-binary @- "$dav_url$review")
[ "$code" = 412 ] || fail "a stale If-Match got $code"
printf 'BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:standup@ministry.example\r\nDTSTAMP:20261120T090000Z\r\nDTSTART:20261123T093000Z\r\nDTEND:20261123T094500Z\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n' | \
  curl -sf -u "$user:$password" -X PUT -H 'If-None-Match: *' --data-binary @- "$dav_url/dav/calendar/home/$calendar/standup.ics" >/dev/null || fail "server-side new event"
sync
grep -q 'review with legal' "$work/cal/$calendar/review.ics" || fail "the change did not come down"
grep -rq 'Standup' "$work/cal/$calendar/" || fail "the new event did not come down"

# A time-range query finds the weekly event in its third week only.
curl -sf -u "$user:$password" -X REPORT -H 'Depth: 1' -H 'Content-Type: application/xml' --data-binary \
  '<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="20261207T000000Z" end="20261208T000000Z"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>' \
  "$dav_url/dav/calendar/home/$calendar/" >"$work/query.xml"
grep -q 'standup.ics' "$work/query.xml" && ! grep -q "$review" "$work/query.xml" || { cat "$work/query.xml" >&2; fail "time-range query"; }

# Deletions go both ways (vdirsyncer never empties a collection by sync).
rm "$work/card/$book/anna.vcf"
curl -sf -u "$user:$password" -X DELETE "$dav_url$review" >/dev/null || fail "server-side delete"
sync
code=$(curl -s -o /dev/null -w '%{http_code}' -u "$user:$password" "$dav_url$anna")
[ "$code" = 404 ] || fail "the contact was not deleted on the server ($code)"
[ ! -e "$work/cal/$calendar/review.ics" ] || fail "the event was not deleted in the app"
code=$(curl -s -o /dev/null -w '%{http_code}' -u "$user:$password" -X PUT --data-binary 'BEGIN:VCARD' "$dav_url/dav/people/home/directory/x.vcf")
[ "$code" = 403 ] || fail "the directory took a card ($code)"
echo "dav-client: CalDAV and CardDAV work with vdirsyncer"
