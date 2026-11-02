package main

import (
	"context"
	"fmt"
	"net"
	"os"
	"sync"

	"github.com/go-sql-driver/mysql"
	"github.com/jmoiron/sqlx"
	"github.com/labstack/echo/v4"
)

const (
	pdnsDBUserEnvKey = "ISUCON13_PDNS_DIALCONFIG_USER"
	pdnsDBPassEnvKey = "ISUCON13_PDNS_DIALCONFIG_PASSWORD"
	pdnsDBNameEnvKey = "ISUCON13_PDNS_DIALCONFIG_DATABASE"
	pdnsZoneName     = "u.isucon.dev"
	// TTL 30 lets PowerDNS packet-cache the answer and lets the bench cache it.
	pdnsRecordTTL = 30
)

var (
	pdnsConn     *sqlx.DB
	pdnsDomainID int64
	pdnsDomainMu sync.Mutex
)

func connectPDNS(logger echo.Logger) (*sqlx.DB, error) {
	conf := mysql.NewConfig()
	conf.Net = "tcp"
	conf.Addr = net.JoinHostPort("127.0.0.1", "3306")
	conf.User = "isudns"
	conf.Passwd = "isudns"
	conf.DBName = "isudns"
	conf.ParseTime = true

	if addr, ok := os.LookupEnv("ISUCON13_MYSQL_DIALCONFIG_ADDRESS"); ok {
		port := "3306"
		if p, ok2 := os.LookupEnv("ISUCON13_MYSQL_DIALCONFIG_PORT"); ok2 {
			port = p
		}
		conf.Addr = net.JoinHostPort(addr, port)
	}
	if v, ok := os.LookupEnv(pdnsDBUserEnvKey); ok {
		conf.User = v
	}
	if v, ok := os.LookupEnv(pdnsDBPassEnvKey); ok {
		conf.Passwd = v
	}
	if v, ok := os.LookupEnv(pdnsDBNameEnvKey); ok {
		conf.DBName = v
	}

	db, err := sqlx.Open("mysql", conf.FormatDSN())
	if err != nil {
		return nil, err
	}
	db.SetMaxOpenConns(10)

	if err := db.Ping(); err != nil {
		return nil, err
	}

	logger.Infof("connected to PowerDNS gmysql %s", conf.DBName)
	return db, nil
}

func resetPDNSDomainID() {
	pdnsDomainMu.Lock()
	pdnsDomainID = 0
	pdnsDomainMu.Unlock()
}

func loadPDNSDomainID(ctx context.Context) (int64, error) {
	pdnsDomainMu.Lock()
	defer pdnsDomainMu.Unlock()
	if pdnsDomainID != 0 {
		return pdnsDomainID, nil
	}
	var id int64
	if err := pdnsConn.GetContext(ctx, &id, "SELECT id FROM domains WHERE name = ?", pdnsZoneName); err != nil {
		return 0, fmt.Errorf("lookup PowerDNS domain %s: %w", pdnsZoneName, err)
	}
	pdnsDomainID = id
	return id, nil
}

func addUserARecord(ctx context.Context, username string) error {
	id, err := loadPDNSDomainID(ctx)
	if err != nil {
		return err
	}
	name := username + "." + pdnsZoneName
	res, err := pdnsConn.ExecContext(ctx,
		"INSERT INTO records (domain_id, name, type, content, ttl, disabled, auth) VALUES (?, ?, 'A', ?, ?, 0, 1)",
		id, name, powerDNSSubdomainAddress, pdnsRecordTTL,
	)
	if err != nil {
		return fmt.Errorf("insert PowerDNS A record %s: %w", name, err)
	}
	n, err := res.RowsAffected()
	if err != nil {
		return err
	}
	if n == 0 {
		return fmt.Errorf("insert PowerDNS A record %s: no rows inserted", name)
	}
	return nil
}
