package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/allegro/bigcache/v3"
	"github.com/jmoiron/sqlx"
	"github.com/labstack/echo/v4"
)

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

var tagsCache *bigcache.BigCache
var themesCache *bigcache.BigCache

func init() {
	var err error
	tagsCache, err = bigcache.New(context.Background(), bigcache.DefaultConfig(0))
	if err != nil {
		panic(err)
	}

	themesCache, err = bigcache.New(context.Background(), bigcache.DefaultConfig(0))
	if err != nil {
		panic(err)
	}
}

func fetchTags(ctx context.Context, tx *sqlx.Tx) ([]byte, error) {
	tagsBytes, err := tagsCache.Get("tags")
	if err != nil {
		var tagModels []*TagModel
		if err := tx.SelectContext(ctx, &tagModels, "SELECT * FROM tags"); err != nil {
			return nil, err
		}
		tags := make([]*Tag, len(tagModels))
		for i := range tagModels {
			tags[i] = &Tag{
				ID:   tagModels[i].ID,
				Name: tagModels[i].Name,
			}
		}
		tagsResponse := TagsResponse{
			Tags: tags,
		}
		tagsBytes, err = json.Marshal(tagsResponse)
		if err != nil {
			return nil, err
		}
		if err := tagsCache.Set("tags", tagsBytes); err != nil {
			return nil, err
		}
	}
	return tagsBytes, nil
}

func getTagHandler(c echo.Context) error {
	ctx := c.Request().Context()

	tx, err := dbConn.BeginTxx(ctx, nil)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to begin transaction: "+err.Error())
	}
	defer tx.Rollback()

	tagsBytes, err := fetchTags(ctx, tx)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to fetch tags: "+err.Error())
	}

	return c.JSONBlob(http.StatusOK, tagsBytes)
}

func fetchTagsByID(ctx context.Context, tx *sqlx.Tx, tagID int64) (*Tag, error) {
	tags, err := fetchTags(ctx, tx)
	if err != nil {
		return nil, err
	}
	var res TagsResponse
	if err := json.Unmarshal(tags, &res); err != nil {
		return nil, err
	}

	for _, tag := range res.Tags {
		if tag.ID == tagID {
			return tag, nil
		}
	}
	return nil, nil
}

func fetchTagByName(ctx context.Context, tx *sqlx.Tx, tagName string) ([]*Tag, error) {
	tags, err := fetchTags(ctx, tx)
	if err != nil {
		return nil, err
	}
	var res TagsResponse
	if err := json.Unmarshal(tags, &res); err != nil {
		return nil, err
	}

	r := make([]*Tag, 0, len(res.Tags))
	for _, tag := range res.Tags {
		if tag.Name == tagName {
			r = append(r, tag)
		}
	}
	return r, nil
}

func fetchThemeByUserID(ctx context.Context, tx *sqlx.Tx, userID int64) (ThemeModel, error) {
	IDKey := fmt.Sprintf("%d", userID)

	var themeModel ThemeModel
	themesBytes, err := themesCache.Get(IDKey)
	if err != nil {
		themeModel := ThemeModel{}
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

		themesBytes, err = json.Marshal(themeModel)
		if err != nil {
			return themeModel, err
		}

		if err := themesCache.Set(IDKey, themesBytes); err != nil {
			return themeModel, err
		}
	}

	return themeModel, nil
}

func fetchUserTheme(ctx context.Context, tx *sqlx.Tx, userName string) (ThemeModel, error) {
	id, err := getUserIDByName(ctx, tx, userName)
	if err != nil {
		return ThemeModel{}, err
	}
	return fetchThemeByUserID(ctx, tx, id)
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
	themeModel, err := fetchUserTheme(ctx, tx, username)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to fetch theme: "+err.Error())
	}

	theme := Theme{
		ID:       themeModel.UserID,
		DarkMode: themeModel.DarkMode,
	}

	return c.JSON(http.StatusOK, theme)
}
