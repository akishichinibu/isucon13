#!/usr/bin/env bash
# Replace pdnsutil load-zone with a bulk INSERT into PowerDNS gmysql.
# Also (re)creates records(name, type) — the official image drops nametype_index.

set -eux
cd $(dirname $0)

if test -f /home/isucon/env.sh; then
	. /home/isucon/env.sh
fi

ISUCON_SUBDOMAIN_ADDRESS=${ISUCON13_POWERDNS_SUBDOMAIN_ADDRESS:-127.0.0.1}
ISUCON_DB_HOST=${ISUCON13_MYSQL_DIALCONFIG_ADDRESS:-127.0.0.1}
ISUCON_DB_PORT=${ISUCON13_MYSQL_DIALCONFIG_PORT:-3306}

PDNS_DB_USER=${ISUCON13_PDNS_DIALCONFIG_USER:-isudns}
PDNS_DB_PASSWORD=${ISUCON13_PDNS_DIALCONFIG_PASSWORD:-isudns}
PDNS_DB_NAME=${ISUCON13_PDNS_DIALCONFIG_DATABASE:-isudns}

mysql_pdns() {
	mysql -u"$PDNS_DB_USER" \
		-p"$PDNS_DB_PASSWORD" \
		--host "$ISUCON_DB_HOST" \
		--port "$ISUCON_DB_PORT" \
		"$PDNS_DB_NAME" "$@"
}

# Idempotent: official provisioning runs `DROP INDEX nametype_index on records`.
mysql_pdns <<'SQL'
SET @exists := (
	SELECT COUNT(1)
	FROM information_schema.statistics
	WHERE table_schema = DATABASE()
	  AND table_name = 'records'
	  AND index_name = 'nametype_index'
);
SET @sql := IF(@exists = 0, 'CREATE INDEX nametype_index ON records (name, type)', 'SELECT 1');
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;
SQL

sql_file=$(mktemp)
trap 'rm -f "$sql_file"' EXIT

awk -v addr="$ISUCON_SUBDOMAIN_ADDRESS" -v ttl=30 \
	-f zone_to_sql.awk u.isucon.dev.zone > "$sql_file"

mysql_pdns < "$sql_file"

# Drop packet/query cache so initialize does not serve the previous run.
curl -sS -X PUT -H 'X-API-Key: isudns' \
	'http://127.0.0.1:8081/api/v1/servers/localhost/cache/flush?domain=u.isucon.dev' \
	>/dev/null || true
