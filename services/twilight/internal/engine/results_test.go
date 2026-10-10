package engine

import (
	"testing"
	"time"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/kafka"
)

func TestOnlyStartedKeepsANodesDispatchInFlight(test *testing.T) {
	engine := &Engine{Dependencies: Dependencies{Config: config.Default()}, leadership: &Leadership{}, evaluations: newEvaluationQueue(10)}
	engine.dispatcher = newDispatcher(engine)
	key := NodeKey{DeviceID: "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", InstallationID: "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"}
	first, second := campaign.Pid(12808937078074471924), campaign.Pid(17352987455937552665)
	engine.dispatcher.inFlight[key] = &batch{pids: map[campaign.Pid]bool{first: true, second: true}, deadline: time.Now().Add(time.Hour)}
	engine.finishInFlight(key, first, kafka.ProcessResult{Status: "started"})
	engine.finishInFlight(key, second, kafka.ProcessResult{Status: "started"})
	if !engine.dispatcher.inFlightFor(key) {
		test.Fatal("started released the node's dispatch")
	}
	engine.finishInFlight(key, first, kafka.ProcessResult{Status: "running"})
	if !engine.dispatcher.inFlightFor(key) {
		test.Fatal("one process's final word released a dispatch whose other process is still out")
	}
	engine.finishInFlight(key, second, kafka.ProcessResult{Status: "duplicate"})
	if engine.dispatcher.inFlightFor(key) {
		test.Fatal("the node's dispatch is still in flight after every process's final word, a duplicate included")
	}
}
