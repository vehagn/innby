#!/usr/bin/env bash
# The benchmark `just bench` runs: a release build of Innby, the site on :9080 and the
# admin UI on :9081, against a database of its own (innby_bench, beside DATABASE_URL's)
# with the sample data and bench/data.sql in it. Each page gets BENCH_DURATION of
# requests from oha: BENCH_CONCURRENCY at once on the site, which a link shared in a
# big chat can send a crowd to, and BENCH_ADMIN_CONCURRENCY at once on the admin UI,
# which a few hosts use, behind a login. The processes' peak memory is read at the end.
#
# It fails if a page answers anything but what it should, or if either process's peak
# memory passes BENCH_MEMORY_BUDGET_MIB: the chart's 64 MiB limit less 8 MiB of room.
# The peak includes a login's 19 MiB argon2 check, made last. Speed is reported, not
# checked, since it depends on the machine. The results are in target/bench/results.md.
set -euo pipefail

binary=${1:?usage: bench/run.sh BINARY}
duration=${BENCH_DURATION:-10s}
concurrency=${BENCH_CONCURRENCY:-32}
admin_concurrency=${BENCH_ADMIN_CONCURRENCY:-4}
budget=${BENCH_MEMORY_BUDGET_MIB:-56}
out=target/bench
site=http://127.0.0.1:9080
admin=http://127.0.0.1:9081
wedding=00000000-be4c-7000-8000-000000000001
command -v oha >/dev/null || { echo "bench needs oha: cargo binstall oha" >&2; exit 1; }
mkdir -p "$out"

echo "Loading the sample and the benchmark's data into innby_bench" >&2
db="${DATABASE_URL%/*}/innby_bench"
psql "$DATABASE_URL" -qc 'drop database if exists innby_bench' -c 'create database innby_bench'
DATABASE_URL=$db "$binary" migrate 2>/dev/null
psql "$db" -q -v ON_ERROR_STOP=1 -1 -f seed/sample.sql -f bench/data.sql

# The example settings, as in development: they're refused only on a public site.
set -a
# shellcheck source=/dev/null
. ./.env.example
set +a
export DATABASE_URL=$db INNBY_BASE_URL=$site INNBY_LISTEN=127.0.0.1:9080 \
  INNBY_ADMIN_LISTEN=127.0.0.1:9081 RUST_LOG=info
# Logging is part of the cost of a request, so it's on, but not kept.
"$binary" serve 2>/dev/null & serve=$!
"$binary" admin 2>/dev/null & admin_pid=$!
trap 'kill $serve $admin_pid 2>/dev/null || true' EXIT
for _ in $(seq 100); do
  curl -sf $site/healthz >/dev/null && curl -sf $admin/healthz >/dev/null && break
  sleep 0.1
done
curl -sf $site/healthz >/dev/null || { echo "the site didn't start" >&2; exit 1; }

log_in() {
  curl -s -o /dev/null -D - -d "user=$1&password=innby" -H 'Sec-Fetch-Site: same-origin' \
    $admin/login | sed -n 's/^set-cookie: \(token=[^;]*\).*/\1/ip' | head -1
}
host=$(log_in admin)
ola=$(log_in ola)
link=$(DATABASE_URL=$db "$binary" invite --event $wedding 'Bench Gjest' 2 2>/dev/null \
  | grep -o '/r/[^ ]*' | head -1)
guest=$(psql "$db" -tAc "select id from rsvp where name = 'Bench Gjest'")
group=$(curl -s -H "Cookie: $host" $admin/groups/00000000-be4c-7000-8000-000000001001 \
  | grep -o 'readonly value="[^"]*/join/[^"]*' | sed 's#.*/join/#/join/#' | head -1)
# Ten forged "remember me" cookies, the most the front page reads.
remember=$(for i in $(seq 0 9); do printf 'innby-0000000%d-0000-7000-8000-000000000000=x.y; ' $i; done)
form=(-H 'Sec-Fetch-Site: same-origin' -H 'Content-Type: application/x-www-form-urlencoded')

results=$out/results.tsv
: > "$results"
failed=0
# A page: its name, the status it should answer with, then oha's arguments.
page() {
  local name=$1 status=$2 at_once=$concurrency; shift 2
  [[ $name == Admin:* ]] && at_once=$admin_concurrency
  echo "  $name" >&2
  oha --no-tui -z "$duration" -c "$at_once" --output-format json "$@" > "$out/run.json" 2>/dev/null
  jq -r --arg name "$name" --arg status "$status" '
    [$name, (.summary.requestsPerSec | floor),
     (.latencyPercentiles.p50 * 1000 | . * 100 | round / 100),
     (.latencyPercentiles.p99 * 1000 | . * 100 | round / 100),
     ((.statusCodeDistribution[$status] // 0) / ([.statusCodeDistribution[]] | add // 1) * 100 | floor)]
    | @tsv' "$out/run.json" | tee -a "$results" | awk -F'\t' '$5 < 100 {exit 1}' || {
    echo "    answered $(jq -c .statusCodeDistribution "$out/run.json"), not only $status" >&2
    failed=1
  }
}

echo "Each page for $duration, $concurrency at once on the site, $admin_concurrency on the admin UI:" >&2
page 'Site: front page'                         200 $site/
page 'Site: front page, 10 forged cookies'      200 -H "Cookie: $remember" $site/
page 'Site: stylesheet'                         200 $site/static/style.css
page 'Site: privacy notice'                     200 $site/privacy
page 'Site: guest page'                         200 $site$link
page 'Site: guest list (1500 guests)'           200 $site$link/guests
page 'Site: guest export'                       200 $site$link/export
page 'Site: group sign-up page'                 200 $site$group
page 'Site: forged link'                        404 $site/r/AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAA
page 'Site: answering'                          303 -m POST "${form[@]}" \
  -d 'response=attending&party_size=2' $site$link
page 'Admin: events (54)'                       200 -H "Cookie: $host" $admin/
page 'Admin: Guests tab (1500 guests)'          200 -H "Cookie: $host" $admin/events/$wedding
page 'Admin: Guests tab, with a role for it'    200 -H "Cookie: $ola" $admin/events/$wedding
page 'Admin: invitation links (1000)'           200 -H "Cookie: $host" $admin/events/$wedding/invitations
page 'Admin: group links'                       200 -H "Cookie: $host" $admin/events/$wedding/groups
page 'Admin: guests CSV (1500 guests)'          200 -H "Cookie: $host" $admin/events/$wedding/guests.csv
page 'Admin: a guest QR code'                   200 -H "Cookie: $host" $admin/guests/$guest/qr.svg
page 'Admin: QR code style'                     200 -H "Cookie: $host" $admin/events/$wedding/qr-style
page 'Admin: not logged in'                     303 $admin/events/$wedding
# Last, so the peak includes a login's argon2 check on top of all that.
log_in admin >/dev/null

# The most each process held at any point, and what it holds now.
memory() { awk -v key="$2" '$1 == key":" {printf "%.1f", $2 / 1024}' "/proc/$1/status"; }
over=0
memory_rows=""
for process in serve admin; do
  pid=$serve; [[ $process == admin ]] && pid=$admin_pid
  peak=$(memory "$pid" VmHWM) now=$(memory "$pid" VmRSS)
  memory_rows+="| \`innby $process\` | $peak MiB | $now MiB |"$'\n'
  awk -v peak="$peak" -v budget="$budget" 'BEGIN {exit !(peak > budget)}' && over=1
done

{
  echo "## Benchmark of $("$binary" --version)"
  echo
  echo "$(date -u +%F), on $(nproc) CPUs, $(uname -m). Each page for $duration: $concurrency requests at once on the site, $admin_concurrency on the admin UI."
  echo
  echo "| Page | Requests/s | Median | Slowest 1% | Answered as it should |"
  echo "|---|---:|---:|---:|---:|"
  awk -F'\t' '{printf "| %s | %s | %s ms | %s ms | %s%% |\n", $1, $2, $3, $4, $5}' "$results"
  echo
  echo "| Process | Peak memory | At the end | "
  echo "|---|---:|---:|"
  printf '%s' "$memory_rows"
  echo
  echo "Budget: $budget MiB each, of the chart's 64 MiB limit."
} > "$out/results.md"
rm -f "$out/run.json"
cat "$out/results.md"

if (( over )); then echo "A process passed the $budget MiB memory budget." >&2; fi
if (( failed )); then echo "A page answered other than it should." >&2; fi
exit $(( over || failed ))
