#!/usr/bin/env bash

set -eux
cd $(dirname $0)

if test -f /home/isucon/env.sh; then
	. /home/isucon/env.sh
fi

ISUCON_DB_HOST=${ISUCON13_MYSQL_DIALCONFIG_ADDRESS:-127.0.0.1}
ISUCON_DB_PORT=${ISUCON13_MYSQL_DIALCONFIG_PORT:-3306}
ISUCON_DB_USER=${ISUCON13_MYSQL_DIALCONFIG_USER:-isucon}
ISUCON_DB_PASSWORD=${ISUCON13_MYSQL_DIALCONFIG_PASSWORD:-isucon}
ISUCON_DB_NAME=${ISUCON13_MYSQL_DIALCONFIG_DATABASE:-isupipe}

# Recreate schema and load dumps in one session. unique_checks/fk checks off
# keeps bulk INSERT fast even with the extra secondary indexes.
mysql -u"$ISUCON_DB_USER" \
		-p"$ISUCON_DB_PASSWORD" \
		--host "$ISUCON_DB_HOST" \
		--port "$ISUCON_DB_PORT" \
		--default-character-set=utf8mb4 \
		"$ISUCON_DB_NAME" <<EOF
source init.sql
source initdb.d/10_schema.sql
source initial_users.sql
source initial_livestreams.sql
source initial_tags.sql
source initial_livestream_tags.sql
source initial_reservation_slots.sql
source initial_reactions.sql
source initial_ngwords.sql
source initial_livecomments.sql
source 11_schema_app.sql
COMMIT;
SET UNIQUE_CHECKS=1;
SET FOREIGN_KEY_CHECKS=1;
EOF

bash ../pdns/init_zone.sh
