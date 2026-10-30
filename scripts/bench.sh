#!/usr/bin/env bash
# usage: [FORGE_BENCH_FILE=path] scripts/bench.sh <project-dir> [runs=5] [binary=target/release/forge]  (builds release if binary missing)
set -u
project=${1:?usage: scripts/bench.sh <project-dir> [runs] [binary]}
runs=${2:-5}
root=$(cd "$(dirname "$0")/.." && pwd)
bin=${3:-$root/target/release/forge}
[ -x "$bin" ] || (cd "$root" && cargo build --release) || exit 1
[ -x "$bin" ] || { echo "no binary at $bin"; exit 1; }

median() { sort -n | awk '{a[NR]=$1} END{if(NR==0){print "n/a"}else if(NR%2){printf "%.1f\n",a[(NR+1)/2]}else{printf "%.1f\n",(a[NR/2]+a[NR/2+1])/2}}'; }
cpu_secs() { ps -o time= -p "$1" | awk -F'[:]' '{n=NF; s=$n; m=$(n-1); h=(n>2)?$(n-2):0; printf "%.2f", h*3600+m*60+s}'; }
mark_ms() { awk -v pat="$2" 'index($0,pat){print $2; exit}' "$1"; }

ff=(); op=(); ix=(); gr=(); rss=(); cpu=()
for i in $(seq 1 "$runs"); do
  log=$(mktemp)
  FORGE_TRACE_STARTUP=1 "$bin" "$project" ${FORGE_BENCH_FILE:+"$FORGE_BENCH_FILE"} 2>"$log" >/dev/null &
  pid=$!
  for _ in $(seq 1 100); do
    grep -q "index ready" "$log" && grep -q "first frame drawn" "$log" && break
    sleep 0.1
  done
  sleep 3
  rss+=("$(ps -o rss= -p "$pid" | awk '{printf "%.1f", $1/1024}')")
  c0=$(cpu_secs "$pid"); sleep 10; c1=$(cpu_secs "$pid")
  cpu+=("$(awk -v a="$c0" -v b="$c1" 'BEGIN{printf "%.2f", (b-a)/10*100}')")
  v=$(mark_ms "$log" "first frame drawn"); ff+=("${v:-}")
  v=$(mark_ms "$log" "open_paths returned"); op+=("${v:-}")
  v=$(mark_ms "$log" "index ready"); ix+=("${v:-}")
  v=$(mark_ms "$log" "git ready"); gr+=("${v:-}")
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  cp "$log" "${TMPDIR:-/tmp}/forge-bench-last.log"; rm -f "$log"
done

med() { printf '%s\n' "$@" | grep -v '^$' | median; }
size=$(stat -f %z "$bin" | awk '{printf "%.1f", $1/1048576}')
echo "project:            $project"
echo "runs:               $runs"
echo "first frame (ms):   $(med "${ff[@]}")"
echo "open_paths (ms):   $(med "${op[@]}")"
echo "git ready (ms):     $(med "${gr[@]}")"
echo "index ready (ms):   $(med "${ix[@]}")"
echo "rss settled (MB):   $(med "${rss[@]}")"
echo "idle cpu 10s (%):   $(med "${cpu[@]}")"
echo "binary size (MB):   $size"
