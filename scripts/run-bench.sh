#!/usr/bin/env bash
# 60s ISUCON13 load. Bench talks DNS to isucon13-1 then HTTPS to resolved A.
set -euo pipefail
NS="${NS:-192.168.0.214}"
BENCH="${BENCH:-192.168.0.217}"
WEB2="${WEB2:-192.168.0.215}"
WEB3="${WEB3:-192.168.0.216}"
ssh -o BatchMode=yes -o ServerAliveInterval=30 ubuntu@"$BENCH" \
  "sudo -u isucon /home/isucon/bench_linux_amd64 run \
    --target https://pipe.u.isucon.dev \
    --nameserver ${NS} \
    --enable-ssl \
    --webapp ${WEB2} --webapp ${WEB3} \
    $*"
