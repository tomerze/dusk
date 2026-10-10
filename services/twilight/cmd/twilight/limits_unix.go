//go:build unix

package main

import (
	"log/slog"
	"syscall"
)

func raiseFileLimit(logger *slog.Logger) {
	var limit syscall.Rlimit
	if failure := syscall.Getrlimit(syscall.RLIMIT_NOFILE, &limit); failure != nil {
		logger.Warn("the open file limit could not be read", "error", failure)
		return
	}
	if limit.Cur >= limit.Max {
		logger.Info("open file limit", "soft", limit.Cur, "hard", limit.Max)
		return
	}
	previous := limit.Cur
	limit.Cur = limit.Max
	if failure := syscall.Setrlimit(syscall.RLIMIT_NOFILE, &limit); failure != nil {
		logger.Warn("the open file limit could not be raised to its hard limit", "soft", previous, "hard", limit.Max, "error", failure)
		return
	}
	logger.Info("open file limit raised to its hard limit", "previous", previous, "soft", limit.Cur)
}
