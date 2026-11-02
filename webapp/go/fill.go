package main

import (
	"context"
	"fmt"

	"github.com/jmoiron/sqlx"
)

func uniqueInt64(ids []int64) []int64 {
	seen := make(map[int64]struct{}, len(ids))
	out := make([]int64, 0, len(ids))
	for _, id := range ids {
		if _, ok := seen[id]; ok {
			continue
		}
		seen[id] = struct{}{}
		out = append(out, id)
	}
	return out
}

type iconHashRow struct {
	UserID   int64  `db:"user_id"`
	IconHash string `db:"icon_hash"`
}

func loadUsersByIDs(ctx context.Context, tx *sqlx.Tx, ids []int64) (map[int64]User, error) {
	ids = uniqueInt64(ids)
	users := make(map[int64]User, len(ids))
	if len(ids) == 0 {
		return users, nil
	}

	query, args, err := sqlx.In("SELECT * FROM users WHERE id IN (?)", ids)
	if err != nil {
		return nil, err
	}
	var userModels []UserModel
	if err := tx.SelectContext(ctx, &userModels, query, args...); err != nil {
		return nil, err
	}

	query, args, err = sqlx.In("SELECT * FROM themes WHERE user_id IN (?)", ids)
	if err != nil {
		return nil, err
	}
	var themeModels []ThemeModel
	if err := tx.SelectContext(ctx, &themeModels, query, args...); err != nil {
		return nil, err
	}
	themes := make(map[int64]ThemeModel, len(themeModels))
	for _, theme := range themeModels {
		themes[theme.UserID] = theme
	}

	query, args, err = sqlx.In("SELECT user_id, icon_hash FROM icons WHERE user_id IN (?)", ids)
	if err != nil {
		return nil, err
	}
	var iconRows []iconHashRow
	if err := tx.SelectContext(ctx, &iconRows, query, args...); err != nil {
		return nil, err
	}
	iconHashes := make(map[int64]string, len(iconRows))
	for _, row := range iconRows {
		iconHashes[row.UserID] = row.IconHash
	}

	fallbackHash := fallbackIconHash()
	for _, userModel := range userModels {
		theme, ok := themes[userModel.ID]
		if !ok {
			return nil, fmt.Errorf("theme not found for user %d", userModel.ID)
		}
		iconHash, ok := iconHashes[userModel.ID]
		if !ok {
			iconHash = fallbackHash
		}
		users[userModel.ID] = User{
			ID:          userModel.ID,
			Name:        userModel.Name,
			DisplayName: userModel.DisplayName,
			Description: userModel.Description,
			Theme: Theme{
				ID:       theme.ID,
				DarkMode: theme.DarkMode,
			},
			IconHash: iconHash,
		}
	}
	return users, nil
}

func loadTagsByLivestreamIDs(ctx context.Context, tx *sqlx.Tx, livestreamIDs []int64) (map[int64][]Tag, error) {
	livestreamIDs = uniqueInt64(livestreamIDs)
	tagsByLivestream := make(map[int64][]Tag, len(livestreamIDs))
	if len(livestreamIDs) == 0 {
		return tagsByLivestream, nil
	}

	query, args, err := sqlx.In(`
		SELECT lt.livestream_id, t.id, t.name
		FROM livestream_tags lt
		INNER JOIN tags t ON t.id = lt.tag_id
		WHERE lt.livestream_id IN (?)
		ORDER BY lt.id ASC
	`, livestreamIDs)
	if err != nil {
		return nil, err
	}

	var rows []struct {
		LivestreamID int64  `db:"livestream_id"`
		ID           int64  `db:"id"`
		Name         string `db:"name"`
	}
	if err := tx.SelectContext(ctx, &rows, query, args...); err != nil {
		return nil, err
	}
	for _, row := range rows {
		tagsByLivestream[row.LivestreamID] = append(tagsByLivestream[row.LivestreamID], Tag{
			ID:   row.ID,
			Name: row.Name,
		})
	}
	return tagsByLivestream, nil
}

func loadLivestreamsByIDs(ctx context.Context, tx *sqlx.Tx, ids []int64) (map[int64]Livestream, error) {
	ids = uniqueInt64(ids)
	result := make(map[int64]Livestream, len(ids))
	if len(ids) == 0 {
		return result, nil
	}

	query, args, err := sqlx.In("SELECT * FROM livestreams WHERE id IN (?)", ids)
	if err != nil {
		return nil, err
	}
	var models []LivestreamModel
	if err := tx.SelectContext(ctx, &models, query, args...); err != nil {
		return nil, err
	}
	livestreams, err := fillLivestreamsResponse(ctx, tx, models)
	if err != nil {
		return nil, err
	}
	for _, livestream := range livestreams {
		result[livestream.ID] = livestream
	}
	return result, nil
}

func fillLivestreamsResponse(ctx context.Context, tx *sqlx.Tx, models []LivestreamModel) ([]Livestream, error) {
	livestreams := make([]Livestream, len(models))
	if len(models) == 0 {
		return livestreams, nil
	}

	ownerIDs := make([]int64, len(models))
	livestreamIDs := make([]int64, len(models))
	for i, model := range models {
		ownerIDs[i] = model.UserID
		livestreamIDs[i] = model.ID
	}

	owners, err := loadUsersByIDs(ctx, tx, ownerIDs)
	if err != nil {
		return nil, err
	}
	tagsByLivestream, err := loadTagsByLivestreamIDs(ctx, tx, livestreamIDs)
	if err != nil {
		return nil, err
	}

	for i, model := range models {
		owner, ok := owners[model.UserID]
		if !ok {
			return nil, fmt.Errorf("owner not found for livestream %d", model.ID)
		}
		tags := tagsByLivestream[model.ID]
		if tags == nil {
			tags = []Tag{}
		}
		livestreams[i] = Livestream{
			ID:           model.ID,
			Owner:        owner,
			Title:        model.Title,
			Description:  model.Description,
			PlaylistUrl:  model.PlaylistUrl,
			ThumbnailUrl: model.ThumbnailUrl,
			Tags:         tags,
			StartAt:      model.StartAt,
			EndAt:        model.EndAt,
		}
	}
	return livestreams, nil
}

func fillLivecommentsResponse(ctx context.Context, tx *sqlx.Tx, models []LivecommentModel) ([]Livecomment, error) {
	livecomments := make([]Livecomment, len(models))
	if len(models) == 0 {
		return livecomments, nil
	}

	userIDs := make([]int64, len(models))
	livestreamIDs := make([]int64, len(models))
	for i, model := range models {
		userIDs[i] = model.UserID
		livestreamIDs[i] = model.LivestreamID
	}

	users, err := loadUsersByIDs(ctx, tx, userIDs)
	if err != nil {
		return nil, err
	}
	livestreams, err := loadLivestreamsByIDs(ctx, tx, livestreamIDs)
	if err != nil {
		return nil, err
	}

	for i, model := range models {
		user, ok := users[model.UserID]
		if !ok {
			return nil, fmt.Errorf("user not found for livecomment %d", model.ID)
		}
		livestream, ok := livestreams[model.LivestreamID]
		if !ok {
			return nil, fmt.Errorf("livestream not found for livecomment %d", model.ID)
		}
		livecomments[i] = Livecomment{
			ID:         model.ID,
			User:       user,
			Livestream: livestream,
			Comment:    model.Comment,
			Tip:        model.Tip,
			CreatedAt:  model.CreatedAt,
		}
	}
	return livecomments, nil
}

func fillReactionsResponse(ctx context.Context, tx *sqlx.Tx, models []ReactionModel) ([]Reaction, error) {
	reactions := make([]Reaction, len(models))
	if len(models) == 0 {
		return reactions, nil
	}

	userIDs := make([]int64, len(models))
	livestreamIDs := make([]int64, len(models))
	for i, model := range models {
		userIDs[i] = model.UserID
		livestreamIDs[i] = model.LivestreamID
	}

	users, err := loadUsersByIDs(ctx, tx, userIDs)
	if err != nil {
		return nil, err
	}
	livestreams, err := loadLivestreamsByIDs(ctx, tx, livestreamIDs)
	if err != nil {
		return nil, err
	}

	for i, model := range models {
		user, ok := users[model.UserID]
		if !ok {
			return nil, fmt.Errorf("user not found for reaction %d", model.ID)
		}
		livestream, ok := livestreams[model.LivestreamID]
		if !ok {
			return nil, fmt.Errorf("livestream not found for reaction %d", model.ID)
		}
		reactions[i] = Reaction{
			ID:         model.ID,
			EmojiName:  model.EmojiName,
			User:       user,
			Livestream: livestream,
			CreatedAt:  model.CreatedAt,
		}
	}
	return reactions, nil
}

func fillLivecommentReportsResponse(ctx context.Context, tx *sqlx.Tx, models []LivecommentReportModel) ([]LivecommentReport, error) {
	reports := make([]LivecommentReport, len(models))
	if len(models) == 0 {
		return reports, nil
	}

	reporterIDs := make([]int64, len(models))
	livecommentIDs := make([]int64, len(models))
	for i, model := range models {
		reporterIDs[i] = model.UserID
		livecommentIDs[i] = model.LivecommentID
	}

	reporters, err := loadUsersByIDs(ctx, tx, reporterIDs)
	if err != nil {
		return nil, err
	}

	livecommentIDs = uniqueInt64(livecommentIDs)
	query, args, err := sqlx.In("SELECT * FROM livecomments WHERE id IN (?)", livecommentIDs)
	if err != nil {
		return nil, err
	}
	var livecommentModels []LivecommentModel
	if err := tx.SelectContext(ctx, &livecommentModels, query, args...); err != nil {
		return nil, err
	}
	livecomments, err := fillLivecommentsResponse(ctx, tx, livecommentModels)
	if err != nil {
		return nil, err
	}
	livecommentsByID := make(map[int64]Livecomment, len(livecomments))
	for _, livecomment := range livecomments {
		livecommentsByID[livecomment.ID] = livecomment
	}

	for i, model := range models {
		reporter, ok := reporters[model.UserID]
		if !ok {
			return nil, fmt.Errorf("reporter not found for livecomment report %d", model.ID)
		}
		livecomment, ok := livecommentsByID[model.LivecommentID]
		if !ok {
			return nil, fmt.Errorf("livecomment not found for report %d", model.ID)
		}
		reports[i] = LivecommentReport{
			ID:          model.ID,
			Reporter:    reporter,
			Livecomment: livecomment,
			CreatedAt:   model.CreatedAt,
		}
	}
	return reports, nil
}
