package main

import (
	"github.com/puzpuzpuz/xsync/v3"
)

var iconImageHashCache = xsync.NewMapOf[int64, string]()

var iconImageCache = xsync.NewMapOf[int64, []byte]()

func Copy(src []byte) []byte {
	dst := make([]byte, len(src))
	copy(dst, src)
	return dst
}
