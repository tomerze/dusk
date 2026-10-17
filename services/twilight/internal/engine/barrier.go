package engine

import (
	"context"
	"fmt"

	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"
)

type resultsBarrier struct {
	engine  *Engine
	client  *kgo.Client
	ends    map[int32]int64
	reached bool
}

func newResultsBarrier(engine *Engine, client *kgo.Client) *resultsBarrier {
	return &resultsBarrier{engine: engine, client: client}
}

func (barrier *resultsBarrier) passed(operation context.Context) (bool, error) {
	if barrier.reached {
		return true, nil
	}
	admin := kadm.NewClient(barrier.client)
	topic, group := barrier.engine.Config.Kafka.Topics.ProcessResults, barrier.engine.Config.Kafka.ResultsGroup
	if barrier.ends == nil {
		listed, failure := admin.ListEndOffsets(operation, topic)
		if failure != nil {
			return false, fmt.Errorf("list the end offsets of %s: %w", topic, failure)
		}
		ends := map[int32]int64{}
		var problem error
		listed.Each(func(offset kadm.ListedOffset) {
			if offset.Err != nil {
				problem = fmt.Errorf("the end offset of %s[%d]: %w", topic, offset.Partition, offset.Err)
			}
			ends[offset.Partition] = offset.Offset
		})
		if problem != nil {
			return false, problem
		}
		barrier.ends = ends
	}
	committed, failure := admin.FetchOffsets(operation, group)
	if failure != nil {
		return false, fmt.Errorf("fetch the offsets group %s committed: %w", group, failure)
	}
	for partition, end := range barrier.ends {
		if end == 0 {
			continue
		}
		offset, found := committed.Lookup(topic, partition)
		if !found || offset.Err != nil || offset.At < end {
			return false, nil
		}
	}
	barrier.reached = true
	barrier.engine.Logger.Info("the process results produced before gates were judged again are applied; gates open phases again", "topic", topic)
	return true, nil
}
