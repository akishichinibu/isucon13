# isupipe (ISUCON13)

Homelab practice on t5820. Baseline score **4688**, official Go reference, `pass=true`.

Winner NaruseJun **468006**. Score is total tips (ISUCOIN) posted during the 60s load.

## Constraints

- `POST /api/initialize` must run `webapp/sql/init.sh` (MySQL dumps + `pdns/init_zone.sh`). Timeout **42s**. Response `language` must match the running impl (`golang` or `rust`).
- bcrypt cost is frozen (`bcrypt.MinCost` / cost 4). Do not change the algorithm or cost.
- Icon `GET /api/user/:username/icon`: If-None-Match vs `icon_hash` **MAY** 304. Unconditional GET must be 200. Hash vs bytes must agree within 2s after POST `/api/icon`.
- DNS: bench talks UDP/53 to nameserver `.214`. Registered usernames must resolve as `<name>.u.isucon.dev` A to a listed webapp IP. Water-torture NXDOMAIN must not kill the nameserver. TTL 0 on pipe/test001 is what the reference zone uses; raising TTL is allowed if pretest still passes.
- `ISUCON13_POWERDNS_SUBDOMAIN_ADDRESS` is the LAN IPv4 of that app VM.
- Session cookie domain `u.isucon.dev` must work on `*.u.isucon.dev`.
- Comments in English.

## Hot paths (reference)

- `GET /api/user/:username/statistics` and `GET /api/livestream/:id/statistics`: load every user/livestream then per-row COUNT/SUM (N+1 ranking). Viewer profile visits time out on this.
- `fillUserResponse`: `SELECT image FROM icons` + SHA256 of the JPEG on every embed (comments, reactions, livestream owner).
- `GET /api/user/:username/icon`: no 304, BLOB from MySQL.
- `POST /api/register`: `exec pdnsutil add-record` per user.
- `POST /api/livestream/:id/moderate`: SELECT all livecomments, then per-row LIKE DELETE.
- `POST /api/livestream/:id/livecomment`: NG-word check via SQL `CONCAT('%', word, '%') LIKE` per word.
- `fillLivestreamResponse` / `fillLivecommentResponse` / `fillReactionResponse`: nested N+1.
- Echo `Debug=true` + request logger. `MaxOpenConns=10`. PowerDNS `cache-ttl=0`. Almost no secondary indexes (`icons.user_id`, `livestreams.user_id`, `livecomments.livestream_id`, …).
- Schema only indexes `ng_words.word`.

## Cluster

| Host | IP | Role |
|------|----|------|
| isucon13-1 | 192.168.0.214 | nginx + Go + MySQL + PowerDNS (bench nameserver) |
| isucon13-2 | 192.168.0.215 | same stack |
| isucon13-3 | 192.168.0.216 | same stack |
| isucon13-bench | 192.168.0.217 | official bench |

HTTPS `https://pipe.u.isucon.dev` via DNS A on `.214`. Keep VM 110/111 and ISUCON14 140–143 stopped.

## Deploy / bench (orchestrator only)

Subagents do not SSH to the VMs. Commit on the current branch.
Orchestrator runs `bash scripts/deploy.sh` then `bash scripts/run-bench.sh`.
