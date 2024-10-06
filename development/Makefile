up/%:
	docker compose -f docker-compose-common.yml -f docker-compose-$*.yml up -d --build

down/%:
	docker compose -f docker-compose-common.yml -f docker-compose-$*.yml down --volumes
	rm -r log || true

log/%:
	docker compose -f docker-compose-common.yml -f docker-compose-$*.yml logs -f | grep webapp
