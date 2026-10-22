#!/bin/bash
# usage: abx.sh <label> <secs> <mitm script args...>
label=$1; secs=$2; shift 2; rm -f all.log mitm.log
./venv12/bin/mitmdump -q --mode reverse:https://api-uk.zap.stream -p 8999 "$@" -s rec.py -s all.py > mitm.out 2>&1 & MP=$!; sleep 3
./player https://localhost:8999/537a365c-f1ec-44ac-af10-22d14a7319fb/hls/live.m3u8 $secs > player.$label.out; rc=$?
kill $MP; sleep 1
echo "===== $label exit=$rc blocking_errs=$(grep -c "Invalid server blocking" player.$label.out) 416_errs=$(grep -c "HTTP 416" player.$label.out) other_errlog=$(grep ERRLOG player.$label.out | grep -vcE "Invalid server blocking|HTTP 416|http2 ECN") stalls=$(grep -c STALLED player.$label.out) proxy_416=$(grep -c " 416 " mitm.log)"
grep SUMMARY player.$label.out
