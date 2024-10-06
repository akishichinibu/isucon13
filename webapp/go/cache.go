package main

import (
	cache "github.com/Code-Hex/go-generics-cache"
)

func emptyCache[K comparable, V any](c *cache.Cache[K, V]) {
	for _, k := range c.Keys() {
		c.Delete(k)
	}
}

type UserCache struct {
	IDToUser           *cache.Cache[int64, UserModel]
	UserNameToID       *cache.Cache[string, int64]
	IDToImageHash      *cache.Cache[int64, string]
	IDToTheme          *cache.Cache[int64, ThemeModel]
	ImageIDToImageHash *cache.Cache[int64, string]
}

func NewUserCache() *UserCache {
	return &UserCache{
		IDToUser:           cache.New[int64, UserModel](),
		UserNameToID:       cache.New[string, int64](),
		IDToImageHash:      cache.New[int64, string](),
		IDToTheme:          cache.New[int64, ThemeModel](),
		ImageIDToImageHash: cache.New[int64, string](),
	}
}

func (m *UserCache) Clear() {
	emptyCache(m.IDToUser)
	emptyCache(m.UserNameToID)
	emptyCache(m.IDToImageHash)
	emptyCache(m.IDToTheme)
	emptyCache(m.ImageIDToImageHash)
}

var userCache = NewUserCache()
