package engine

import (
	"context"
	"fmt"
	"math/rand/v2"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/kafka"
)

var censusSkips = promauto.NewCounter(prometheus.CounterOpts{
	Name: "twilight_connections_skips_total",
	Help: "Times the connection stream was skipped to its latest offsets because it fell behind.",
})

func (engine *Engine) partitions(operation context.Context, admin *kadm.Client, topic string) ([]int32, error) {
	details, failure := admin.ListTopics(operation, topic)
	if failure != nil {
		return nil, failure
	}
	detail, found := details[topic]
	if !found || detail.Err != nil {
		return nil, fmt.Errorf("topic %s is not available: %v", topic, detail.Err)
	}
	return detail.Partitions.Numbers(), nil
}

func (engine *Engine) runPresence(operation context.Context) {
	backoff := time.Second
	for operation.Err() == nil {
		failure := engine.consumePresence(operation)
		if operation.Err() != nil {
			return
		}
		engine.presenceUp.Store(false)
		engine.Logger.Error("the presence stream stopped; restarting it", "error", failure)
		sleep(operation, time.Duration(rand.Int64N(int64(backoff)))+1)
		backoff = min(2*backoff, time.Minute)
	}
}

func (engine *Engine) consumePresence(operation context.Context) error {
	topics := engine.Config.Kafka.Topics
	client, failure := kgo.NewClient(append(append([]kgo.Opt(nil), engine.KafkaOptions...), kgo.FetchMaxWait(time.Second))...)
	if failure != nil {
		return failure
	}
	defer client.Close()
	admin := kadm.NewClient(client)
	censusPartitions, failure := engine.partitions(operation, admin, topics.Census)
	if failure != nil {
		return failure
	}
	censusEnds, failure := admin.ListEndOffsets(operation, topics.Census)
	if failure != nil {
		return fmt.Errorf("census end offsets: %w", failure)
	}
	assignment := map[int32]kgo.Offset{}
	remaining := map[int32]int64{}
	for _, partition := range censusPartitions {
		assignment[partition] = kgo.NewOffset().AtStart()
		if end, found := censusEnds.Lookup(topics.Census, partition); found && end.Offset > 0 {
			remaining[partition] = end.Offset
		}
	}
	client.AddConsumePartitions(map[string]map[int32]kgo.Offset{topics.Census: assignment})
	engine.Logger.Info("reading the census", "topic", topics.Census, "partitions", len(censusPartitions))
	idle := 0
	for len(remaining) > 0 && idle < 2 {
		polling, cancel := context.WithTimeout(operation, 3*time.Second)
		fetches := client.PollFetches(polling)
		cancel()
		if operation.Err() != nil {
			return operation.Err()
		}
		if failure := kafka.FetchFailure(fetches); failure != nil {
			return failure
		}
		records := 0
		fetches.EachRecord(func(record *kgo.Record) {
			records++
			engine.applyRecord(record)
			if end, waiting := remaining[record.Partition]; waiting && record.Offset+1 >= end {
				delete(remaining, record.Partition)
			}
		})
		if records == 0 {
			idle++
		} else {
			idle = 0
		}
	}
	oldest := engine.view.OldestHeader(time.Now())
	start := oldest.Add(-60 * time.Second)
	if oldest.IsZero() {
		start = time.Now().Add(-time.Duration(engine.Config.Engine.CensusIntervalSeconds)*time.Second - 60*time.Second)
	}
	connectionPartitions, failure := engine.partitions(operation, admin, topics.Connections)
	if failure != nil {
		return failure
	}
	offsets, failure := admin.ListOffsetsAfterMilli(operation, start.UnixMilli(), topics.Connections)
	if failure != nil {
		return fmt.Errorf("connection offsets after %s: %w", start, failure)
	}
	connectionAssignment := map[int32]kgo.Offset{}
	for _, partition := range connectionPartitions {
		offset := kgo.NewOffset().AtEnd()
		if listed, found := offsets.Lookup(topics.Connections, partition); found && listed.Err == nil && listed.Offset >= 0 {
			offset = kgo.NewOffset().At(listed.Offset)
		}
		connectionAssignment[partition] = offset
	}
	client.AddConsumePartitions(map[string]map[int32]kgo.Offset{topics.Connections: connectionAssignment})
	engine.presenceUp.Store(true)
	engine.Logger.Info("the online view is loaded; following connections", "since", start.UTC().Format(time.RFC3339), "online", engine.view.OnlineCount())
	for {
		fetches := client.PollFetches(operation)
		if operation.Err() != nil {
			return operation.Err()
		}
		if failure := kafka.FetchFailure(fetches); failure != nil {
			return failure
		}
		var lag int64
		var newest time.Time
		fetches.EachPartition(func(partition kgo.FetchTopicPartition) {
			if partition.Topic != topics.Connections || len(partition.Records) == 0 {
				return
			}
			last := partition.Records[len(partition.Records)-1]
			lag += partition.HighWatermark - last.Offset - 1
			if last.Timestamp.After(newest) {
				newest = last.Timestamp
			}
		})
		fetches.EachRecord(engine.applyRecord)
		if fallenBehind(lag, newest, time.Now(), engine.Config.Engine.SkipLagRecords, time.Duration(engine.Config.Engine.SkipAgeSeconds)*time.Second) {
			engine.skipToLatest(operation, client, admin, topics.Connections, lag, newest)
		}
	}
}

func fallenBehind(lag int64, newest, now time.Time, maximumLag int, maximumAge time.Duration) bool {
	return lag > int64(maximumLag) || (lag > 0 && !newest.IsZero() && now.Sub(newest) > maximumAge)
}

func (engine *Engine) skipToLatest(operation context.Context, client *kgo.Client, admin *kadm.Client, topic string, lag int64, newest time.Time) {
	ends, failure := admin.ListEndOffsets(operation, topic)
	if failure != nil {
		engine.Logger.Warn("skipping to the latest connections failed", "error", failure)
		return
	}
	target := map[string]map[int32]kgo.EpochOffset{topic: {}}
	ends.Each(func(listed kadm.ListedOffset) {
		target[topic][listed.Partition] = kgo.EpochOffset{Epoch: -1, Offset: listed.Offset}
	})
	client.SetOffsets(target)
	engine.view.MarkSkipped()
	censusSkips.Inc()
	engine.Logger.Warn("the connection stream fell behind; skipped to its latest offsets and the view is degraded until every census has refreshed it",
		"lag", lag, "newest_record", newest.UTC().Format(time.RFC3339))
}

func (engine *Engine) applyRecord(record *kgo.Record) {
	if record.Value == nil {
		return
	}
	switch record.Topic {
	case engine.Config.Kafka.Topics.Census:
		message, valid := kafka.Decode[kafka.CensusRecord](engine.Validator, kafka.ContractCensus, record, engine.Logger)
		if valid {
			engine.onChanges(engine.view.ApplyCensus(message))
		}
	case engine.Config.Kafka.Topics.Connections:
		message, valid := kafka.Decode[kafka.Connection](engine.Validator, kafka.ContractConnections, record, engine.Logger)
		if valid {
			engine.onChanges(engine.view.ApplyConnection(message))
		}
	}
}
