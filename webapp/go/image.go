package main

import (
	"context"
	"crypto/sha256"
	"fmt"
	"os"

	"github.com/allegro/bigcache/v3"
)

var fallbackImage = "../img/NoImage.jpg"
var fallbackImageID string
var fallbackImageData []byte

var imageStore *bigcache.BigCache

func dumpImage(data []byte) (string, error) {
	hash := sha256.Sum256(data)
	ID := fmt.Sprintf("%x", hash)
	err := os.WriteFile(fmt.Sprintf("../img/%s.jpg", ID), data, 0644)
	// err := imageStore.Set(ID, data)
	return ID, err
}

func loadImage(ID string) ([]byte, error) {
	data, err := os.ReadFile(fmt.Sprintf("../img/%s.jpg", ID))
	// data, err := imageStore.Get(ID)
	if err != nil {
		return fallbackImageData, nil
	}
	return data, nil
}

func init() {
	var err error
	imageStore, err = bigcache.New(context.Background(), bigcache.DefaultConfig(0))
	if err != nil {
		panic(err)
	}
	{
		fallbackImageData, err = os.ReadFile(fallbackImage)
		if err != nil {
			panic(err)
		}
		h := sha256.Sum256(fallbackImageData)
		fallbackImageID = fmt.Sprintf("%x", h)
		err = imageStore.Set(fallbackImageID, fallbackImageData)
		if err != nil {
			panic(err)
		}
	}
}
