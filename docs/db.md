# Schema notes (reference)

Tables: users, icons (LONGBLOB), themes, livestreams, reservation_slots, tags,
livestream_tags, livestream_viewers_history, livecomments, livecomment_reports,
ng_words, reactions.

Only index in 10_schema.sql: `ng_words.word`.

Add at least:

- `icons(user_id)` unique + `icon_hash CHAR(64)` so fillUser skips the BLOB
- `livestreams(user_id)`, `livestream_tags(livestream_id)`, `livestream_tags(tag_id)`
- `livecomments(livestream_id)`, `reactions(livestream_id)`, `livecomment_reports(livestream_id)`
- `livestream_viewers_history(livestream_id)`, `themes(user_id)`
- PowerDNS `records(name, type)` if still using gmysql

`sqlx` `SELECT *` must match struct fields after adding columns.

Initialize reloads dumps then `pdnsutil load-zone`. Keep it under 42s.
