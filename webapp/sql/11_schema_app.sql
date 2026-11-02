-- Applied on every initialize so existing data dirs pick up API-required columns.

ALTER TABLE icons ADD COLUMN IF NOT EXISTS icon_hash CHAR(64) NOT NULL DEFAULT '';

SET @idx_exists := (
  SELECT COUNT(1)
  FROM information_schema.statistics
  WHERE table_schema = DATABASE()
    AND table_name = 'icons'
    AND index_name = 'uniq_icons_user_id'
);
SET @sql := IF(@idx_exists = 0, 'ALTER TABLE icons ADD UNIQUE KEY uniq_icons_user_id (user_id)', 'SELECT 1');
PREPARE stmt FROM @sql;
EXECUTE stmt;
DEALLOCATE PREPARE stmt;

UPDATE icons SET icon_hash = SHA2(image, 256) WHERE icon_hash = '';
