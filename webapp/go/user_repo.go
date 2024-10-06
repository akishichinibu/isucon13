package main

import (
	"context"
	"os"

	"github.com/jmoiron/sqlx"
)

var imageRoot = "/home/isucon/webapp/img/"

func repoLoadImage(imageHash string) ([]byte, error) {
	bs, err := os.ReadFile(imageRoot + imageHash)
	if err != nil {
		return nil, err
	}
	return bs, nil
}

func repoDumpImage(imageHash string, data []byte) error {
	return os.WriteFile(imageRoot+imageHash, data, 0644)
}

func repoGetThemeByUserID(ctx context.Context, tx *sqlx.Tx, userID int64) (ThemeModel, error) {
	themeModel, ok := userCache.IDToTheme.Get(userID)
	if ok {
		return themeModel, nil
	}
	if err := tx.GetContext(ctx, &themeModel, "SELECT * FROM themes WHERE user_id = ?", userID); err != nil {
		return themeModel, err
	}
	userCache.IDToTheme.Set(userID, themeModel)
	return themeModel, nil
}

func repoGetImageHashByUserID(ctx context.Context, tx *sqlx.Tx, userID int64) (string, error) {
	imageHash, ok := userCache.IDToImageHash.Get(userID)
	if ok {
		return imageHash, nil
	}
	if err := tx.GetContext(ctx, &imageHash, "SELECT image FROM icons WHERE user_id = ? LIMIT 1", userID); err != nil {
		return imageHash, err
	}
	userCache.IDToImageHash.Set(userID, imageHash)
	return imageHash, nil
}

func repoGetUserByID(ctx context.Context, tx *sqlx.Tx, userID int64) (UserModel, error) {
	userModel, ok := userCache.IDToUser.Get(userID)
	if ok {
		return userModel, nil
	}
	if err := tx.GetContext(ctx, &userModel, "SELECT * FROM users WHERE id = ?", userID); err != nil {
		return userModel, err
	}
	userCache.IDToUser.Set(userID, userModel)
	return userModel, nil
}

func repoGetUserIDByUserName(ctx context.Context, tx *sqlx.Tx, userName string) (int64, error) {
	userID, ok := userCache.UserNameToID.Get(userName)
	if ok {
		return userID, nil
	}
	var userModel UserModel
	if err := tx.GetContext(ctx, &userModel, "SELECT * FROM users WHERE name = ?", userName); err != nil {
		return 0, err
	}
	userCache.UserNameToID.Set(userName, userModel.ID)
	return userModel.ID, nil
}
