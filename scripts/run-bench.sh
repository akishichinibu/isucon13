#!/usr/bin/env bash
# 60s ISUCON13 load. Bench talks DNS to .215 then HTTPS to resolved A.
# A records are the two nginx TLS edges; rust app stays on .215.
set -euo pipefail
NS="${NS:-192.168.0.215}"
BENCH="${BENCH:-192.168.0.217}"
EDGE1="${EDGE1:-192.168.0.214}"
EDGE2="${EDGE2:-192.168.0.216}"
ssh -o BatchMode=yes -o ServerAliveInterval=30 ubuntu@"$BENCH" \
  "sudo -u isucon /home/isucon/bench_linux_amd64 run \
    --target https://pipe.u.isucon.dev \
    --nameserver ${NS} \
    --enable-ssl \
    --webapp ${EDGE1} --webapp ${EDGE2} \
    $*"
