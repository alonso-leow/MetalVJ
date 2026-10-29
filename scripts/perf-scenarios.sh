#!/usr/bin/env bash
# Launch the real app (window and all), then walk it through scenarios and
# record, per scenario, CPU% per thread (from /proc deltas) plus `GET /perf`.
#
#   ./scripts/perf-scenarios.sh [out_dir] [seconds_per_scenario]
#
# Scenarios: startup (time until /perf answers), idle, then `/select` of each
# model in $MODELS. Uses target/release/metal-dash; port 7977 so it never
# clashes with a running instance. stderr goes to out_dir/app.log.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/target/perf-scenarios}"
SECS="${2:-12}"
PORT=7977
MODELS="${MODELS:-aurora fineline corals monster orbit-swarm water}"
BIN="${BIN:-$ROOT/target/release/metal-dash}"
mkdir -p "$OUT"

ticks() { # tid -> utime+stime for every thread of $1, "name ticks"
  for t in /proc/"$1"/task/*; do
    [ -r "$t/stat" ] || continue
    local s; s=$(<"$t/stat") || continue
    local name=${s#*(}; name=${name%%)*}
    local rest=${s##*) }; set -- $rest
    echo "${name// /_} $(( ${12} + ${13} ))"
  done
}

sample() { # pid seconds label
  local a b hz; hz=$(getconf CLK_TCK)
  a=$(ticks "$1" | sort); sleep "$2"; b=$(ticks "$1" | sort)
  echo "== $3 (${2}s) CPU% per thread group =="
  join -a2 -e0 -o 0,1.2,2.2 <(echo "$a" | awk '{s[$1]+=$2} END{for(k in s)print k,s[k]}' | sort) \
                          <(echo "$b" | awk '{s[$1]+=$2} END{for(k in s)print k,s[k]}' | sort) |
    awk -v hz="$hz" -v s="$2" '{p=($3-$2)*100/hz/s; tot+=p; if(p>=1) printf "  %-28s %6.1f%%\n",$1,p} END{printf "  %-28s %6.1f%%\n","TOTAL",tot}' |
    sort -k2 -nr -t$'\t'
}

cd "$ROOT"
t0=$(date +%s.%N)
"$BIN" --port "$PORT" >"$OUT/app.log" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
until curl -sf "localhost:$PORT/perf" >/dev/null; do
  kill -0 $PID 2>/dev/null || { echo "app died"; tail -20 "$OUT/app.log"; exit 1; }
  sleep 0.1
done
echo "startup: /perf answered after $(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.2f", b-a}') s"
sleep 3
sample $PID "$SECS" idle | tee "$OUT/idle.txt"
curl -s "localhost:$PORT/perf" >"$OUT/idle.perf.json"
for m in $MODELS; do
  curl -s -XPOST "localhost:$PORT/select" -d "{\"id\":\"$m\"}" >/dev/null
  sleep 2
  # On-air load: pull 720p frames back to back while sampling (render +
  # readback + JPEG, the path a remote monitor/node takes).
  ( end=$((SECONDS + SECS)); n=0
    while [ $SECONDS -lt $end ]; do curl -s -o /dev/null "localhost:$PORT/preview.jpg?w=1280&h=720"; n=$((n+1)); done
    echo "  preview.jpg 720p: $((n / SECS)) fps" >"$OUT/$m.fps" ) &
  sample $PID "$SECS" "$m" | tee "$OUT/$m.txt"
  wait $!; cat "$OUT/$m.fps"
  curl -s "localhost:$PORT/perf" >"$OUT/$m.perf.json"
done
echo "logs: $OUT"
