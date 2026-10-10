//go:build !unix

package main

import "log/slog"

func raiseFileLimit(logger *slog.Logger) {
	logger.Info("the open file limit is left as the platform sets it")
}
