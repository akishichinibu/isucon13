#!/usr/bin/env bash
# Dual TLS: .214 and .216 nginx (A-record targets) -> .215 rust:8080
# .215 rust app + in-process UDP/53 (bench nameserver)
# .216 also keeps MySQL for initialize
# App stays a single in-memory process. Do not colocate rust with nginx.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
SSH=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new)
RSYNC=(rsync -az --delete -e "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new")

deploy_nginx() {
  local host=$1
  echo "==== nginx TLS ${host} ===="
  "${RSYNC[@]}" "$ROOT/webapp/public/" ubuntu@"$host":/tmp/isupipe-public/
  scp -o BatchMode=yes "$ROOT/nginx/isupipe.conf" ubuntu@"$host":/tmp/isupipe.conf
  "${SSH[@]}" ubuntu@"$host" 'sudo bash -s' << 'EOF'
set -euo pipefail
rsync -a --delete /tmp/isupipe-public/ /home/isucon/webapp/public/
chown -R isucon:isucon /home/isucon/webapp/public
systemctl disable --now isupipe-rust isupipe-go pdns || true
install -m 0644 /tmp/isupipe.conf /etc/nginx/sites-enabled/isupipe.conf
sed -i 's/worker_connections [0-9]\+/worker_connections 4096/' /etc/nginx/nginx.conf || true
sed -i 's|^[ \t]*access_log .*|	access_log off;|' /etc/nginx/nginx.conf || true
sed -i 's|^[ \t]*error_log .*|	error_log /dev/null emerg;|' /etc/nginx/nginx.conf || true
: > /var/log/nginx/access.log || true
: > /var/log/nginx/error.log || true
nginx -t
systemctl enable --now nginx
systemctl reload nginx || systemctl restart nginx
systemctl is-active nginx
EOF
}

export RUSTFLAGS="${RUSTFLAGS:--C target-cpu=skylake -C force-frame-pointers=yes}"
echo "==== cargo musl release ===="
(cd "$ROOT/webapp/rust" && cargo build --release --locked --target x86_64-unknown-linux-musl)
BIN="$ROOT/webapp/rust/target/x86_64-unknown-linux-musl/release/isupipe"
test -x "$BIN"

echo "==== mysql 192.168.0.216 ===="
scp -o BatchMode=yes "$ROOT/mysql/mysqld-isucon.cnf" ubuntu@192.168.0.216:/tmp/mysqld-isucon.cnf
"${SSH[@]}" ubuntu@192.168.0.216 'sudo bash -s' << 'EOF'
set -euo pipefail
install -m 0644 /tmp/mysqld-isucon.cnf /etc/mysql/mysql.conf.d/isucon.cnf
if [ -f /etc/mysql/mysql.conf.d/mysqld.cnf ]; then
  sed -i 's/^bind-address/# bind-address/' /etc/mysql/mysql.conf.d/mysqld.cnf || true
  sed -i 's/^mysqlx-bind-address/# mysqlx-bind-address/' /etc/mysql/mysql.conf.d/mysqld.cnf || true
fi
systemctl disable --now isupipe-rust isupipe-go pdns || true
systemctl enable --now mysql
systemctl restart mysql
mysql -uroot -e "
  CREATE DATABASE IF NOT EXISTS isupipe;
  CREATE USER IF NOT EXISTS 'isucon'@'%' IDENTIFIED BY 'isucon';
  CREATE USER IF NOT EXISTS 'isucon'@'localhost' IDENTIFIED BY 'isucon';
  GRANT ALL PRIVILEGES ON *.* TO 'isucon'@'%';
  GRANT ALL PRIVILEGES ON *.* TO 'isucon'@'localhost';
  FLUSH PRIVILEGES;
"
systemctl is-active mysql
EOF

echo "==== rust+dns 192.168.0.215 ===="
"${RSYNC[@]}" "$ROOT/webapp/sql/" ubuntu@192.168.0.215:/tmp/isupipe-sql/
"${RSYNC[@]}" "$ROOT/webapp/pdns/" ubuntu@192.168.0.215:/tmp/isupipe-pdns/
"${RSYNC[@]}" "$ROOT/webapp/img/" ubuntu@192.168.0.215:/tmp/isupipe-img/
scp -o BatchMode=yes "$BIN" ubuntu@192.168.0.215:/tmp/isupipe
scp -o BatchMode=yes "$ROOT/systemd/isupipe-rust.service" ubuntu@192.168.0.215:/tmp/isupipe-rust.service
"${SSH[@]}" ubuntu@192.168.0.215 'sudo bash -s' << 'EOF'
set -euo pipefail
rsync -a --delete /tmp/isupipe-sql/ /home/isucon/webapp/sql/
rsync -a --delete /tmp/isupipe-pdns/ /home/isucon/webapp/pdns/
rsync -a --delete /tmp/isupipe-img/ /home/isucon/webapp/img/
install -d -o isucon -g isucon /home/isucon/webapp/rust
install -m 0755 -o isucon -g isucon /tmp/isupipe /home/isucon/webapp/rust/isupipe
cat > /home/isucon/env.sh << 'ENV'
ISUCON13_MYSQL_DIALCONFIG_NET="tcp"
ISUCON13_MYSQL_DIALCONFIG_ADDRESS="192.168.0.216"
ISUCON13_MYSQL_DIALCONFIG_PORT="3306"
ISUCON13_MYSQL_DIALCONFIG_USER="isucon"
ISUCON13_MYSQL_DIALCONFIG_DATABASE="isupipe"
ISUCON13_MYSQL_DIALCONFIG_PARSETIME="true"
ISUCON13_POWERDNS_SUBDOMAIN_ADDRESS="192.168.0.214,192.168.0.216"
ISUCON13_POWERDNS_DISABLED="true"
ENV
chown isucon:isucon /home/isucon/env.sh
chmod 0755 /home/isucon/env.sh
install -m 0644 /tmp/isupipe-rust.service /etc/systemd/system/isupipe-rust.service
systemctl daemon-reload
systemctl disable --now isupipe-go pdns mysql || true
systemctl enable --now isupipe-rust
systemctl restart isupipe-rust
systemctl is-active isupipe-rust
EOF

deploy_nginx 192.168.0.214
"${SSH[@]}" ubuntu@192.168.0.214 'sudo systemctl disable --now mysql || true'
deploy_nginx 192.168.0.216

echo "cluster deploy done"
echo "nameserver=.215  A=.214,.216  mysql=.216  rust=.215:8080"


