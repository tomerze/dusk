//go:build ui

package web

import (
	"embed"
	"io/fs"
)

//go:embed all:dist
var distribution embed.FS

func Assets() (fs.FS, bool) {
	assets, failure := fs.Sub(distribution, "dist")
	return assets, failure == nil
}
