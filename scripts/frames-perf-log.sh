#!/usr/bin/env bash
# Log fetch+decode ms and dropped frames from `GET /perf` over a LAN run.
#
# This is plan §5 step 3. Its one job is the Raspberry Pi LAN run: a **release
# build on the Pi** with **one remote layer at 720p**, polling the peer's
# `GET /perf` while the link carries frames. The `remote` object (added by
# `leap/frame-relay`) reports cumulative `fetched`/`dropped` counts and the mean
# `fetch_ms`/`decode_ms` since the process started; this script timestamps a
# snapshot every interval so the run can be read back as a csv.
#
# A persistent stream (one long-lived GET) is **only adopted if the Pi cannot
# hold 30 fps** under this polling. Until those numbers say otherwise, one GET
# per frame is the design, so this logger deliberately measures it in place.
#
# Pi-friendliness: bash + `curl` (falls back to `wget`), no `jq`. The `remote`
# object is carved out of the body with `sed` and each field read from that
# slice, so field order and surrounding JSON do not matter. `python3` is *not*
# required.
#
#   ./scripts/frames-perf-log.sh --url 192.168.1.20:7900 --interval 1
#   ./scripts/frames-perf-log.sh --port 7900 --seconds 30 --csv > run.csv
#   ./scripts/frames-perf-log.sh --host 192.168.1.20 --port 7900 --human
#   ./scripts/frames-perf-log.sh --from /tmp/perf.json        # parse a body
#   cat /tmp/perf.json | ./scripts/frames-perf-log.sh --from -  # ... from stdin
#   ./scripts/frames-perf-log.sh --self-test                  # parser check
#
# Exit is 0 on a clean stop; a malformed body (no `remote` object) is a logged
# warning, not a crash — the run keeps polling. `--once`/`--from` exit non-zero
# if their one body cannot be parsed.
set -euo pipefail

HOST="127.0.0.1"
PORT="7900"
INTERVAL="1"
SECONDS_TO_RUN="0"
MODE="csv"
ONCE="0"
FROM=""
TIMEOUT="5"

die() { echo "frames-perf-log: $*" >&2; exit 2; }

usage() { sed -n '2,29p' "$0" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --host) [ $# -ge 2 ] || die "--host needs a value"; HOST="$2"; shift 2 ;;
    --port) [ $# -ge 2 ] || die "--port needs a value"; PORT="$2"; shift 2 ;;
    --url)  [ $# -ge 2 ] || die "--url needs host:port"; HOST="${2%%:*}"; PORT="${2##*:}"; shift 2 ;;
    --interval) [ $# -ge 2 ] || die "--interval needs a value"; INTERVAL="$2"; shift 2 ;;
    --seconds)  [ $# -ge 2 ] || die "--seconds needs a value"; SECONDS_TO_RUN="$2"; shift 2 ;;
    --timeout)  [ $# -ge 2 ] || die "--timeout needs a value"; TIMEOUT="$2"; shift 2 ;;
    --csv) MODE="csv"; shift ;;
    --human) MODE="human"; shift ;;
    --once) ONCE="1"; shift ;;
    --from) [ $# -ge 2 ] || die "--from needs a file (or -)"; FROM="$2"; shift 2 ;;
    --self-test) MODE="csv"; ONCE="1"; SELF_TEST="1"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
done

URL="http://$HOST:$PORT/perf"

fetch_body() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsS --max-time "$TIMEOUT" "$URL"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O - --timeout="$TIMEOUT" "$URL"
  else
    die "need curl or wget to fetch $URL"
  fi
}

# `remote` slice, then one field out of it. Field order is irrelevant: the
# object carries no nested braces, so `[^}]*` stops at its close.
remote=""
remote_of() {
  printf '%s' "$1" | sed -n 's/.*"remote"[[:space:]]*:[[:space:]]*{\([^}]*\)}.*/\1/p'
}
field() {
  printf '%s' "$remote" \
    | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\([^,}]*\).*/\1/p" \
    | tr -d ' \t\r\n'
}

is_num() { case "$1" in ''|*[!0-9.eE+-]*) return 1 ;; *) return 0 ;; esac; }

now_iso()  { date -u +%Y-%m-%dT%H:%M:%SZ; }
now_unix() { date -u +%s; }

# One body -> one line on stdout. Returns 1 if the `remote` object is absent.
log_line() {
  remote="$(remote_of "$1")"
  [ -n "$remote" ] || return 1
  local fetched dropped fetch_ms decode_ms
  fetched="$(field fetched)"
  dropped="$(field dropped)"
  fetch_ms="$(field fetch_ms)"
  decode_ms="$(field decode_ms)"
  for v in "$fetched" "$dropped" "$fetch_ms" "$decode_ms"; do
    is_num "$v" || return 1
  done
  if [ "$MODE" = "human" ]; then
    printf '%s  fetched=%s dropped=%s fetch=%sms decode=%sms\n' \
      "$(now_iso)" "$fetched" "$dropped" "$fetch_ms" "$decode_ms"
  else
    printf '%s,%s,%s,%s,%s,%s\n' \
      "$(now_iso)" "$(now_unix)" "$fetched" "$dropped" "$fetch_ms" "$decode_ms"
  fi
}

# --from (or --self-test) parses a given body instead of hitting the network:
# how the parser is proven without a live server.
if [ -n "$FROM" ]; then
  if [ "$FROM" = "-" ]; then body="$(cat)"; else
    [ -r "$FROM" ] || die "cannot read $FROM"
    body="$(cat "$FROM")"
  fi
  if [ "$MODE" = "csv" ]; then
    echo "# timestamp,unix,fetched,dropped,fetch_ms,decode_ms"
  fi
  log_line "$body" || die "no parsable remote object in $FROM"
  exit 0
fi

if [ "${SELF_TEST:-0}" = "1" ]; then
  want="100,5,2.5,1.25"
  sample='{"window_s":3,"fps":30,"remote":{"decode_ms":1.25,"dropped":5,"fetch_ms":2.5,"fetched":100}}'
  MODE="csv" out="$(log_line "$sample")"
  fields="$(printf '%s' "$out" | cut -d, -f3-6)"
  [ "$fields" = "$want" ] || die "self-test: parsed '$fields', wanted '$want'"
  echo "self-test ok: fetched,dropped,fetch_ms,decode_ms = $fields"
  exit 0
fi

if [ "$MODE" = "csv" ]; then
  echo "# timestamp,unix,fetched,dropped,fetch_ms,decode_ms"
fi
limit=""
[ "$SECONDS_TO_RUN" != 0 ] && limit=", for ${SECONDS_TO_RUN}s"
echo "# polling $URL every ${INTERVAL}s${limit} (release build on the Pi, one remote layer at 720p)" >&2

stop=0
trap 'stop=1' INT TERM
first_dropped=""
last_dropped=""
samples=0
started="$SECONDS"
while [ "$stop" = 0 ]; do
  if body="$(fetch_body 2>/dev/null)"; then
    if line="$(log_line "$body")"; then
      echo "$line"
      remote="$(remote_of "$body")"
      d="$(field dropped)"
      [ -n "$first_dropped" ] || first_dropped="$d"
      last_dropped="$d"
      samples=$((samples + 1))
    else
      echo "frames-perf-log: no parsable remote object (run a frames-pi build)" >&2
    fi
  else
    echo "frames-perf-log: no answer from $URL" >&2
  fi
  [ "$ONCE" = 1 ] && break
  if [ "$SECONDS_TO_RUN" != 0 ] && [ $((SECONDS - started)) -ge "$SECONDS_TO_RUN" ]; then
    break
  fi
  sleep "$INTERVAL" || true
done

if [ "$samples" -gt 0 ]; then
  echo "frames-perf-log: $samples samples, dropped ${first_dropped}->${last_dropped}" >&2
fi
