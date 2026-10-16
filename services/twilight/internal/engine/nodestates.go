package engine

import (
	"context"
	"log/slog"
	"math/rand/v2"
	"time"

	"dusk/services/twilight/internal/nodestate"
)

type NodeStateChecker interface {
	Check(operation context.Context) (nodestate.Report, error)
}

type checkPace struct {
	period  time.Duration
	base    time.Duration
	ceiling time.Duration
}

var nodeStatePace = checkPace{period: time.Minute, base: 5 * time.Second, ceiling: 5 * time.Minute}

func (pace checkPace) retry(failures int) time.Duration {
	limit := pace.ceiling
	if shifted := pace.base << min(failures, 30); shifted > 0 && shifted < limit {
		limit = shifted
	}
	return time.Duration(rand.Int64N(int64(limit))) + 1
}

func (engine *Engine) runNodeStateCheck(operation context.Context, term int64) {
	checkNodeStates(operation, engine.NodeStates, engine.Logger, term, nodeStatePace)
}

func checkNodeStates(operation context.Context, checker NodeStateChecker, logger *slog.Logger, term int64, pace checkPace) {
	failures := 0
	for {
		report, failure := checker.Check(operation)
		wait := pace.period
		switch {
		case failure != nil && operation.Err() != nil:
			return
		case failure != nil:
			failures++
			wait = pace.retry(failures)
			switch {
			case !report.Checked:
				logger.Warn("the node-state topic could not be checked; the check is tried again", "term", term, "error", failure, "retry_seconds", wait.Seconds())
			case report.Rebuilt:
				logger.Warn("the node-state topic was written again from the inventory but the check did not finish; the check is tried again",
					"term", term, "error", failure, "retry_seconds", wait.Seconds())
			case len(report.Incomplete) > 0:
				logger.Error("the node-state topic lacks completion records and could not be rebuilt; nightfall refuses nodes until it is",
					"term", term, "incomplete_partitions", report.Incomplete, "error", failure, "retry_seconds", wait.Seconds())
			default:
				logger.Warn("the node-state topic is complete but its check could not be recorded; the check is tried again", "term", term, "error", failure, "retry_seconds", wait.Seconds())
			}
		case report.Skipped:
			failures = 0
			logger.Debug("the node-state topic is the one last found complete", "term", term)
		case report.Rebuilt:
			failures = 0
			logger.Warn("the node-state topic had partitions without a completion record - a new stack, a topic Kafka lost, or partitions added - and was written again from the inventory",
				"term", term, "incomplete_partitions", report.Incomplete, "published", report.Published, "written_back", report.WrittenBack,
				"unknown_nodes", report.Unknown, "in_sync", report.InSync, "moved", report.Moved)
		default:
			failures = 0
			logger.Info("the node-state topic is complete", "term", term, "partitions", report.Partitions)
		}
		if !sleep(operation, wait) {
			return
		}
	}
}
