#!/usr/bin/env bash
# Deploy webapp/go + sql + pdns to the three app VMs and restart isupipe-go.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
HOSTS=(192.168.0.214 192.168.0.215 192.168.0.216)
SSH=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new)
RSYNC=(rsync -az --delete -e "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new")

for ip in "${HOSTS[@]}"; do
  echo "==== deploy $ip ===="
  "${RSYNC[@]}" "$ROOT/webapp/go/" "ubuntu@${ip}:/tmp/isupipe-go/"
  "${RSYNC[@]}" "$ROOT/webapp/sql/" "ubuntu@${ip}:/tmp/isupipe-sql/"
  "${RSYNC[@]}" "$ROOT/webapp/pdns/" "ubuntu@${ip}:/tmp/isupipe-pdns/"
  "${RSYNC[@]}" "$ROOT/webapp/img/" "ubuntu@${ip}:/tmp/isupipe-img/"
  if [ -f "$ROOT/mysql/mysqld-isucon.cnf" ]; then
    scp -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
      "$ROOT/mysql/mysqld-isucon.cnf" "ubuntu@${ip}:/tmp/mysqld-isucon.cnf"
  fi
  scp -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
    "$ROOT/nginx/isupipe.conf" "ubuntu@${ip}:/tmp/isupipe.conf"
  if [ -f "$ROOT/systemd/isupipe-go.service" ]; then
    scp -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
      "$ROOT/systemd/isupipe-go.service" "ubuntu@${ip}:/tmp/isupipe-go.service"
  fi
  "${SSH[@]}" "ubuntu@${ip}" "sudo bash -s" << 'EOF'
set -euo pipefail
rsync -a --delete /tmp/isupipe-go/ /home/isucon/webapp/go/
rsync -a --delete /tmp/isupipe-sql/ /home/isucon/webapp/sql/
rsync -a --delete /tmp/isupipe-pdns/ /home/isucon/webapp/pdns/
rsync -a --delete /tmp/isupipe-img/ /home/isucon/webapp/img/
chown -R isucon:isucon /home/isucon/webapp/go /home/isucon/webapp/sql /home/isucon/webapp/pdns /home/isucon/webapp/img
if [ -f /home/isucon/webapp/pdns/pdns.conf ]; then
  install -m 0644 /home/isucon/webapp/pdns/pdns.conf /etc/powerdns/pdns.conf
fi
if [ -f /tmp/mysqld-isucon.cnf ]; then
  install -m 0644 /tmp/mysqld-isucon.cnf /etc/mysql/mysql.conf.d/isucon.cnf
  systemctl restart mysql
fi
if [ -f /tmp/isupipe.conf ]; then
  install -m 0644 /tmp/isupipe.conf /etc/nginx/sites-enabled/isupipe.conf
  nginx -t && systemctl reload nginx
fi
if [ -f /tmp/isupipe-go.service ]; then
  install -m 0644 /tmp/isupipe-go.service /etc/systemd/system/isupipe-go.service
  systemctl daemon-reload
fi
if [ -f /home/isucon/webapp/pdns/pdns.conf ]; then
  systemctl restart pdns
fi
sudo -u isucon bash -lc 'cd /home/isucon/webapp/go && /home/isucon/local/golang/bin/go build -mod=vendor -o isupipe -ldflags "-s -w"'
systemctl restart isupipe-go
systemctl is-active isupipe-go nginx mysql pdns
EOF
done
echo "deploy done"
