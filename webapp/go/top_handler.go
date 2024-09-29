package main

import (
	"context"
	"encoding/json"
	"net/http"

	"github.com/allegro/bigcache/v3"
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

func fetchTags(ctx context.Context) ([]byte, error) {
	tagsBytes, err := tagsCache.Get("tags")
	if err != nil {
		tx, err := dbConn.BeginTxx(ctx, nil)
		if err != nil {
			return nil, err
		}
		defer tx.Rollback()

		var tagModels []*TagModel
		if err := tx.SelectContext(ctx, &tagModels, "SELECT * FROM tags"); err != nil {
			return nil, err
		}

		if err := tx.Commit(); err != nil {
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

	tagsBytes, err := fetchTags(ctx)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to fetch tags: "+err.Error())
	}

	return c.JSONBlob(http.StatusOK, tagsBytes)
}

func fetchUserTheme(ctx context.Context, userName string) ([]byte, error) {
	themesBytes, err := themesCache.Get(userName)
	if err != nil {
		tx, err := dbConn.BeginTxx(ctx, nil)
		if err != nil {
			return nil, err
		}
		defer tx.Rollback()

		themeModel := ThemeModel{}
		if err := tx.GetContext(ctx, &themeModel, `
SELECT 
	themes.id AS id,
	themes.user_id AS user_id,
	themes.dark_mode AS dark_mode
FROM themes
INNER JOIN users ON themes.user_id = users.id
WHERE users.name = ?
LIMIT 1
		`, userName); err != nil {
			return nil, err
		}

		// if err := tx.GetContext(ctx, &themeModel, "SELECT * FROM themes WHERE user_id = ?", userModel.ID); err != nil {
		// 	return nil, err
		// }

		theme := Theme{
			ID:       themeModel.ID,
			DarkMode: themeModel.DarkMode,
		}

		themesBytes, err = json.Marshal(theme)
		if err != nil {
			return nil, err
		}

		if err := themesCache.Set(userName, themesBytes); err != nil {
			return nil, err
		}
	}

	return themesBytes, nil
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
	themeBytes, err := fetchUserTheme(ctx, username)
	if err != nil {
		return echo.NewHTTPError(http.StatusInternalServerError, "failed to fetch theme: "+err.Error())
	}

	return c.JSONBlob(http.StatusOK, themeBytes)
}
