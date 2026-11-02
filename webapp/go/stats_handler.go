package main

import (
	"database/sql"
	"errors"
	"net/http"
	"strconv"

	"github.com/labstack/echo/v4"
)

type LivestreamStatistics struct {
	Rank           int64 `json:"rank"`
	ViewersCount   int64 `json:"viewers_count"`
	TotalReactions int64 `json:"total_reactions"`
	TotalReports   int64 `json:"total_reports"`
	MaxTip         int64 `json:"max_tip"`
}

type UserStatistics struct {
	Rank              int64  `json:"rank"`
	ViewersCount      int64  `json:"viewers_count"`
	TotalReactions    int64  `json:"total_reactions"`
	TotalLivecomments int64  `json:"total_livecomments"`
	TotalTip          int64  `json:"total_tip"`
	FavoriteEmoji     string `json:"favorite_emoji"`
}

type userScoreRow struct {
	UserID int64 `db:"user_id"`
	Score  int64 `db:"score"`
}

type livestreamScoreRow struct {
	LivestreamID int64 `db:"livestream_id"`
	Score        int64 `db:"score"`
}

func getUserStatisticsHandler(c echo.Context) error {
	ctx := c.Request().Context()

	if err := verifyUserSession(c); err != nil {
		// echo.NewHTTPErrorが返っているのでそのまま出力
		return err
	}

	username := c.Param("username")
	// ユーザごとに、紐づく配信について、累計リアクション数、累計ライブコメント数、累計売上金額を算出
	// また、現在の合計視聴者数もだす

	tx, err := dbConn.BeginTxx(ctx, nil)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to begin transaction: "+err.Error())
	}
	defer tx.Rollback()

	var user UserModel
	if err := tx.GetContext(ctx, &user, "SELECT * FROM users WHERE name = ?", username); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return echo.NewHTTPError(http.StatusBadRequest, "not found user that has the given username")
		} else {
			return echo.NewHTTPError(http.StatusInternalServerError, "failed to get user: "+err.Error())
		}
	}

	// Rank is 1 + users with a higher (reactions+tips) score; ties break by username DESC.
	var users []struct {
		ID   int64  `db:"id"`
		Name string `db:"name"`
	}
	if err := tx.SelectContext(ctx, &users, "SELECT id, name FROM users"); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to get users: "+err.Error())
	}

	reactionScores := make(map[int64]int64, len(users))
	var reactionRows []userScoreRow
	if err := tx.SelectContext(ctx, &reactionRows, `
		SELECT l.user_id, COUNT(*) AS score
		FROM livestreams l
		INNER JOIN reactions r ON r.livestream_id = l.id
		GROUP BY l.user_id
	`); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to count reactions: "+err.Error())
	}
	for _, row := range reactionRows {
		reactionScores[row.UserID] = row.Score
	}

	tipScores := make(map[int64]int64, len(users))
	var tipRows []userScoreRow
	if err := tx.SelectContext(ctx, &tipRows, `
		SELECT l.user_id, IFNULL(SUM(lc.tip), 0) AS score
		FROM livestreams l
		INNER JOIN livecomments lc ON lc.livestream_id = l.id
		GROUP BY l.user_id
	`); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to count tips: "+err.Error())
	}
	for _, row := range tipRows {
		tipScores[row.UserID] = row.Score
	}

	myScore := reactionScores[user.ID] + tipScores[user.ID]
	var rank int64 = 1
	for _, u := range users {
		score := reactionScores[u.ID] + tipScores[u.ID]
		if score > myScore || (score == myScore && u.Name > username) {
			rank++
		}
	}

	totalReactions := reactionScores[user.ID]

	var commentStats struct {
		Count int64 `db:"cnt"`
		Tip   int64 `db:"tip"`
	}
	if err := tx.GetContext(ctx, &commentStats, `
		SELECT COUNT(*) AS cnt, IFNULL(SUM(lc.tip), 0) AS tip
		FROM livestreams l
		INNER JOIN livecomments lc ON lc.livestream_id = l.id
		WHERE l.user_id = ?
	`, user.ID); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to get livecomments: "+err.Error())
	}

	var viewersCount int64
	if err := tx.GetContext(ctx, &viewersCount, `
		SELECT COUNT(*)
		FROM livestreams l
		INNER JOIN livestream_viewers_history h ON h.livestream_id = l.id
		WHERE l.user_id = ?
	`, user.ID); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to get livestream_view_history: "+err.Error())
	}

	var favoriteEmoji string
	query := `
	SELECT r.emoji_name
	FROM users u
	INNER JOIN livestreams l ON l.user_id = u.id
	INNER JOIN reactions r ON r.livestream_id = l.id
	WHERE u.name = ?
	GROUP BY emoji_name
	ORDER BY COUNT(*) DESC, emoji_name DESC
	LIMIT 1
	`
	if err := tx.GetContext(ctx, &favoriteEmoji, query, username); err != nil && !errors.Is(err, sql.ErrNoRows) {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to find favorite emoji: "+err.Error())
	}

	if err := tx.Commit(); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to commit: "+err.Error())
	}

	stats := UserStatistics{
		Rank:              rank,
		ViewersCount:      viewersCount,
		TotalReactions:    totalReactions,
		TotalLivecomments: commentStats.Count,
		TotalTip:          commentStats.Tip,
		FavoriteEmoji:     favoriteEmoji,
	}
	return c.JSON(http.StatusOK, stats)
}

func getLivestreamStatisticsHandler(c echo.Context) error {
	ctx := c.Request().Context()

	if err := verifyUserSession(c); err != nil {
		return err
	}

	id, err := strconv.Atoi(c.Param("livestream_id"))
	if err != nil {
		return echo.NewHTTPError(http.StatusBadRequest, "livestream_id in path must be integer")
	}
	livestreamID := int64(id)

	tx, err := dbConn.BeginTxx(ctx, nil)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to begin transaction: "+err.Error())
	}
	defer tx.Rollback()

	var livestream LivestreamModel
	if err := tx.GetContext(ctx, &livestream, "SELECT * FROM livestreams WHERE id = ?", livestreamID); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return echo.NewHTTPError(http.StatusBadRequest, "cannot get stats of not found livestream")
		} else {
			return echo.NewHTTPError(http.StatusInternalServerError, "failed to get livestream: "+err.Error())
		}
	}

	var livestreamIDs []int64
	if err := tx.SelectContext(ctx, &livestreamIDs, "SELECT id FROM livestreams"); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to get livestreams: "+err.Error())
	}

	// Rank is 1 + livestreams with a higher (reactions+tips) score; ties break by id DESC.
	reactionScores := make(map[int64]int64, len(livestreamIDs))
	var reactionRows []livestreamScoreRow
	if err := tx.SelectContext(ctx, &reactionRows, `
		SELECT livestream_id, COUNT(*) AS score
		FROM reactions
		GROUP BY livestream_id
	`); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to count reactions: "+err.Error())
	}
	for _, row := range reactionRows {
		reactionScores[row.LivestreamID] = row.Score
	}

	tipScores := make(map[int64]int64, len(livestreamIDs))
	var tipRows []livestreamScoreRow
	if err := tx.SelectContext(ctx, &tipRows, `
		SELECT livestream_id, IFNULL(SUM(tip), 0) AS score
		FROM livecomments
		GROUP BY livestream_id
	`); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to count tips: "+err.Error())
	}
	for _, row := range tipRows {
		tipScores[row.LivestreamID] = row.Score
	}

	myScore := reactionScores[livestreamID] + tipScores[livestreamID]
	var rank int64 = 1
	for _, id := range livestreamIDs {
		score := reactionScores[id] + tipScores[id]
		if score > myScore || (score == myScore && id > livestreamID) {
			rank++
		}
	}

	var viewersCount int64
	if err := tx.GetContext(ctx, &viewersCount, `SELECT COUNT(*) FROM livestream_viewers_history WHERE livestream_id = ?`, livestreamID); err != nil && !errors.Is(err, sql.ErrNoRows) {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to count livestream viewers: "+err.Error())
	}

	var maxTip int64
	if err := tx.GetContext(ctx, &maxTip, `SELECT IFNULL(MAX(tip), 0) FROM livecomments WHERE livestream_id = ?`, livestreamID); err != nil && !errors.Is(err, sql.ErrNoRows) {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to find maximum tip livecomment: "+err.Error())
	}

	var totalReactions int64
	if err := tx.GetContext(ctx, &totalReactions, "SELECT COUNT(*) FROM reactions WHERE livestream_id = ?", livestreamID); err != nil && !errors.Is(err, sql.ErrNoRows) {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to count total reactions: "+err.Error())
	}

	var totalReports int64
	if err := tx.GetContext(ctx, &totalReports, `SELECT COUNT(*) FROM livecomment_reports WHERE livestream_id = ?`, livestreamID); err != nil && !errors.Is(err, sql.ErrNoRows) {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to count total spam reports: "+err.Error())
	}

	if err := tx.Commit(); err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to commit: "+err.Error())
	}

	return c.JSON(http.StatusOK, LivestreamStatistics{
		Rank:           rank,
		ViewersCount:   viewersCount,
		MaxTip:         maxTip,
		TotalReactions: totalReactions,
		TotalReports:   totalReports,
	})
}
