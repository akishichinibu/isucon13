package main

import (
	"context"
	"net/http"

	cache "github.com/Code-Hex/go-generics-cache"
	"github.com/jmoiron/sqlx"
	"github.com/labstack/echo/v4"
)

// MARK: original

var cacheTagIDToTag *cache.Cache[int64, TagModel]
var cacheThemeIDToTheme *cache.Cache[int64, ThemeModel]

func init() {
	cacheTagIDToTag = cache.New[int64, TagModel]()
	cacheThemeIDToTheme = cache.New[int64, ThemeModel]()
}

func fetchTags(ctx context.Context, tx *sqlx.Tx) ([]TagModel, error) {
	tagIDs := cacheTagIDToTag.Keys()
	if len(tagIDs) == 0 {
		var tagModels []TagModel
		if err := tx.SelectContext(ctx, &tagModels, "SELECT * FROM tags"); err != nil {
			return nil, err
		}
		for _, t := range tagModels {
			cacheTagIDToTag.Set(t.ID, t)
		}
	}
	tagIDs = cacheTagIDToTag.Keys()
	tags := make([]TagModel, 0, len(tagIDs))
	for _, id := range tagIDs {
		tag, _ := cacheTagIDToTag.Get(id)
		tags = append(tags, tag)
	}
	return tags, nil
}

func fetchTagByTagID(ctx context.Context, tx *sqlx.Tx, tagID int64) (TagModel, error) {
	tagModel, ok := cacheTagIDToTag.Get(tagID)
	if !ok {
		if err := tx.GetContext(ctx, &tagModel, "SELECT * FROM tags WHERE id = ?", tagID); err != nil {
			return tagModel, err
		}
		cacheTagIDToTag.Set(tagID, tagModel)
		return tagModel, nil
	}
	return tagModel, nil
}

func fetchTagByName(ctx context.Context, tx *sqlx.Tx, tagName string) ([]TagModel, error) {
	tags, err := fetchTags(ctx, tx)
	if err != nil {
		return nil, err
	}
	r := make([]TagModel, 0)
	for _, tag := range tags {
		if tag.Name == tagName {
			r = append(r, tag)
		}
	}
	return r, nil
}

func fetchThemeByUserID(ctx context.Context, tx *sqlx.Tx, userID int64) (ThemeModel, error) {
	theme, ok := cacheThemeIDToTheme.Get(userID)
	if !ok {
		var themeModel ThemeModel
		if err := tx.GetContext(ctx, &themeModel, `
SELECT 
	themes.id AS id,
	themes.user_id AS user_id,
	themes.dark_mode AS dark_mode
FROM themes
WHERE themes.user_id = ?
LIMIT 1
		`, userID); err != nil {
			return themeModel, err
		}
		cacheThemeIDToTheme.Set(userID, themeModel)
		return themeModel, nil
	}

	return theme, nil
}

func fetchUserThemeByUserName(ctx context.Context, tx *sqlx.Tx, userName string) (ThemeModel, error) {
	id, err := getUserIDByName(ctx, tx, userName)
	if err != nil {
		return ThemeModel{}, err
	}
	return fetchThemeByUserID(ctx, tx, id)
}

// MARK: - original

type Tag struct {
	ID   int64  `json:"id"`
	Name string `json:"name"`
}

type TagModel struct {
	ID   int64  `db:"id"`
	Name string `db:"name"`
}

type TagsResponse struct {
	Tags []*Tag `json:"tags"`
}

func getTagHandler(c echo.Context) error {
	ctx := c.Request().Context()

	tx, err := dbConn.BeginTxx(ctx, nil)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to begin transaction: "+err.Error())
	}
	defer tx.Rollback()

	tags, err := fetchTags(ctx, tx)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to fetch tags: "+err.Error())
	}

	var ts TagsResponse
	ts.Tags = make([]*Tag, 0, len(tags))
	for _, tag := range tags {
		ts.Tags = append(ts.Tags, &Tag{
			ID:   tag.ID,
			Name: tag.Name,
		})
	}

	return c.JSON(http.StatusOK, ts)
}

// 配信者のテーマ取得API
// GET /api/user/:username/theme
func getStreamerThemeHandler(c echo.Context) error {
	ctx := c.Request().Context()

	if err := verifyUserSession(c); err != nil {
		// echo.NewHTTPErrorが返っているのでそのまま出力
		c.Logger().Printf("verifyUserSession: %+v\n", err)
		return err
	}

	username := c.Param("username")

	tx, err := dbConn.BeginTxx(ctx, nil)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to begin transaction: "+err.Error())
	}
	defer tx.Rollback()

	themeModel, err := fetchUserThemeByUserName(ctx, tx, username)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to fetch theme: "+err.Error())
	}

	theme := Theme{
		ID:       themeModel.UserID,
		DarkMode: themeModel.DarkMode,
	}

	return c.JSON(http.StatusOK, theme)
}
