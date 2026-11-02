# isupipe

Local ISUCON13 practice tree for the t5820 3+1 cluster.
Baseline is the official Go reference, scored on this homelab.

## Cluster

| Host | IP | Role |
|------|----|------|
| isucon13-1 | 192.168.0.214 | nginx + Go + MySQL + PowerDNS (bench nameserver) |
| isucon13-2 | 192.168.0.215 | same stack |
| isucon13-3 | 192.168.0.216 | same stack |
| isucon13-bench | 192.168.0.217 | official bench |

HTTPS `https://pipe.u.isucon.dev` via PowerDNS on `.214`. Language: Go.

## Scores (60s bench)

| Tree | Result | Score |
|------|--------|-------|
| `main` initial Go reference | pass | **4688** |
| `feat/go-opt` indexes + icon_hash + stats + DNS cache | pass | **38212** |
| `feat/rust` in-memory + UDP/53 | pass | **190642** (DNS 206048/0, viewers 982) |
| `feat/rust` 3-tier + custom HTTP | pass | **261037** (DNS 199711/0, viewers 1331) |
| `feat/rust` slot headroom 256 + cheaper fills | pass | **285441** (viewers 1500, cold-reserve 2864/946) |
| `feat/rust` shards+rank cache, nginx+app colocated | pass | 238645 (TLS and app sharing 2c) |
| `feat/rust` shards+rank cache, split nginx/app | pass | **281576** |

Winner NaruseJun 468006. Score is total tips (ISUCOIN).

```bash
bash scripts/deploy-cluster.sh
bash scripts/run-bench.sh
```

Cluster: `.214` nginx TLS, `.215` rust+DNS, `.216` MySQL. Bench nameserver is `.215`; A records point at `.214`.

## Layout

```
webapp/go/            Go app (running)
webapp/sql/           schema + init dumps
webapp/public/        frontend
webapp/pdns/          zone template + init_zone.sh
webapp/img/           fallback icon
webapp/rust/          official Rust reference (not running)
nginx/                isupipe.conf
systemd/              isupipe-go
mysql/                mysqld drop-in
scripts/deploy.sh     rsync + rebuild + restart on the three app VMs
scripts/run-bench.sh  60s load from the bench VM
```

Keep t5820 VM 110/111 and ISUCON14 140–143 stopped while benching.
