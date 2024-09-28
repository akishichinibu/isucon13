up/%:
	docker compose -f docker-compose-common.yml -f docker-compose-$*.yml up -d --build --force-recreate

down/%:
	docker compose -f docker-compose-common.yml -f docker-compose-$*.yml down --volumes
