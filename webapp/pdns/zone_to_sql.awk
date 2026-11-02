BEGIN {
	print "SET NAMES latin1;"
	print "INSERT INTO domains (name, type) SELECT 'u.isucon.dev', 'NATIVE' FROM DUAL WHERE NOT EXISTS (SELECT 1 FROM domains WHERE name='u.isucon.dev');"
	print "SET @did := (SELECT id FROM domains WHERE name='u.isucon.dev');"
	print "DELETE FROM records WHERE domain_id=@did;"
	print "INSERT INTO records (domain_id, name, type, content, ttl, disabled, auth) VALUES"
	printf "(@did,'u.isucon.dev','SOA','ns1.u.isucon.dev hostmaster.u.isucon.dev 0 10800 3600 604800 3600',3600,0,1)"
}
/^\$/ { next }
/^[ \t]*;/ { next }
/^[ \t]*$/ { next }
$1 == "@" && $2 == "SOA" { skip = 1; next }
skip {
	if (index($0, ")") > 0) skip = 0
	next
}
{
	name = $1
	type = $4
	content = $5
	if (name == "@") {
		fqdn = "u.isucon.dev"
	} else {
		fqdn = name ".u.isucon.dev"
	}
	gsub(/\.$/, "", content)
	if (content == "<ISUCON_SUBDOMAIN_ADDRESS>") content = addr
	printf ",\n(@did,'%s','%s','%s',%d,0,1)", fqdn, type, content, ttl
}
END {
	print ";"
}
