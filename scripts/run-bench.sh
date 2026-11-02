#!/usr/bin/env bash
# 60s ISUCON13 load. Bench talks DNS to isucon13-1 then HTTPS to resolved A.
set -euo pipefail
# Nameserver is the rust box (.215). A records point at nginx (.214).
NS="${NS:-192.168.0.215}"
BENCH="${BENCH:-192.168.0.217}"
EDGE="${EDGE:-192.168.0.214}"
ssh -o BatchMode=yes -o ServerAliveInterval=30 ubuntu@"$BENCH" \
  "sudo -u isucon /home/isucon/bench_linux_amd64 run \
    --target https://pipe.u.isucon.dev \
    --nameserver ${NS} \
    --enable-ssl \
    --webapp ${EDGE} \
    $*"
