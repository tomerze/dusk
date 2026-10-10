//go:build !ui

package web

import "io/fs"

func Assets() (fs.FS, bool) {
	return nil, false
}
