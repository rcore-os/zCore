#!/bin/sh
# drmbench comparison runner: same QEMU, two kernels, one diff.
#
# Usage (inside each guest):   ./drmbench /dev/dri/card0 5 > /tmp/result.txt
# Then, on the host:           ./run-compare.sh eclipse.txt linux.txt
#
# Prints every metric side by side with the ratio, so a regression or a win is
# visible without reading two files. Metrics only one side reports (e.g. one
# kernel answering "unsupported") are shown too — an unimplemented ioctl is a
# comparison result, not a gap in the data.
A="${1:?usage: run-compare.sh <eclipse.txt> <linux.txt>}"
B="${2:?usage: run-compare.sh <eclipse.txt> <linux.txt>}"
printf '%-32s %14s %14s %8s\n' METRIC "$(basename "$A" .txt)" "$(basename "$B" .txt)" RATIO
printf '%-32s %14s %14s %8s\n' '--------------------------------' -------------- -------------- --------
for k in $(cat "$A" "$B" | sed 's/=.*//' | sort -u); do
  va=$(grep "^$k=" "$A" 2>/dev/null | head -1 | sed 's/^[^=]*=//')
  vb=$(grep "^$k=" "$B" 2>/dev/null | head -1 | sed 's/^[^=]*=//')
  na=$(printf '%s' "$va" | awk '{print $1+0}')
  nb=$(printf '%s' "$vb" | awk '{print $1+0}')
  r=$(awk -v a="$na" -v b="$nb" 'BEGIN{ if (b>0) printf "%.2fx", a/b; else print "-" }')
  printf '%-32s %14s %14s %8s\n' "$k" "${va:--}" "${vb:--}" "$r"
done
