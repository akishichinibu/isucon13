#!/usr/bin/env bash
# Build isupipe on this machine (ubuntu-dev) and deploy the binary to the three app VMs.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
HOSTS=(192.168.0.214 192.168.0.215 192.168.0.216)
SSH=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new)
RSYNC=(rsync -az --delete -e "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new")

# ubuntu-dev is Ubuntu 24.04 (glibc 2.39); guests are jammy (2.35).
# Static musl binary plus Skylake CPU (W-2145), not the i5-13600T host.
export RUSTFLAGS="${RUSTFLAGS:--C target-cpu=skylake}"
echo "==== cargo build --release --target x86_64-unknown-linux-musl ===="
(cd "$ROOT/webapp/rust" && cargo build --release --locked --target x86_64-unknown-linux-musl)
BIN="$ROOT/webapp/rust/target/x86_64-unknown-linux-musl/release/isupipe"
test -x "$BIN"

for ip in "${HOSTS[@]}"; do
  echo "==== deploy $ip ===="
  "${RSYNC[@]}" "$ROOT/webapp/sql/" "ubuntu@${ip}:/tmp/isupipe-sql/"
  "${RSYNC[@]}" "$ROOT/webapp/pdns/" "ubuntu@${ip}:/tmp/isupipe-pdns/"
  "${RSYNC[@]}" "$ROOT/webapp/img/" "ubuntu@${ip}:/tmp/isupipe-img/"
  scp -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
    "$BIN" "ubuntu@${ip}:/tmp/isupipe"
  if [ -f "$ROOT/mysql/mysqld-isucon.cnf" ]; then
    scp -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
      "$ROOT/mysql/mysqld-isucon.cnf" "ubuntu@${ip}:/tmp/mysqld-isucon.cnf"
  fi
  scp -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
    "$ROOT/nginx/isupipe.conf" "ubuntu@${ip}:/tmp/isupipe.conf"
  scp -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
    "$ROOT/systemd/isupipe-rust.service" "ubuntu@${ip}:/tmp/isupipe-rust.service"
  "${SSH[@]}" "ubuntu@${ip}" "sudo bash -s" << 'EOF'
set -euo pipefail
rsync -a --delete /tmp/isupipe-sql/ /home/isucon/webapp/sql/
rsync -a --delete /tmp/isupipe-pdns/ /home/isucon/webapp/pdns/
rsync -a --delete /tmp/isupipe-img/ /home/isucon/webapp/img/
install -d -o isucon -g isucon /home/isucon/webapp/rust
install -m 0755 -o isucon -g isucon /tmp/isupipe /home/isucon/webapp/rust/isupipe
chown -R isucon:isucon /home/isucon/webapp/sql /home/isucon/webapp/pdns /home/isucon/webapp/img /home/isucon/webapp/rust
if [ -f /tmp/mysqld-isucon.cnf ]; then
  install -m 0644 /tmp/mysqld-isucon.cnf /etc/mysql/mysql.conf.d/isucon.cnf
  systemctl restart mysql
fi
if [ -f /tmp/isupipe.conf ]; then
  install -m 0644 /tmp/isupipe.conf /etc/nginx/sites-enabled/isupipe.conf
  nginx -t && systemctl reload nginx
fi
install -m 0644 /tmp/isupipe-rust.service /etc/systemd/system/isupipe-rust.service
systemctl daemon-reload
systemctl disable --now isupipe-go || true
systemctl disable --now pdns || true
systemctl enable --now isupipe-rust
systemctl is-active isupipe-rust nginx mysql
EOF
done
echo "deploy done"
