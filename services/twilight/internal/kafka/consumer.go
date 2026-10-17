package kafka

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"math/rand/v2"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/jackc/pgx/v5/pgconn"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/twmb/franz-go/pkg/kgo"
)

func FetchFailure(fetches kgo.Fetches) error {
	for _, fetchError := range fetches.Errors() {
		if errors.Is(fetchError.Err, context.DeadlineExceeded) {
			continue
		}
		if errors.Is(fetchError.Err, context.Canceled) {
			return fetchError.Err
		}
		return fmt.Errorf("fetch %s[%d]: %w", fetchError.Topic, fetchError.Partition, fetchError.Err)
	}
	return nil
}

var refusedRecords = promauto.NewCounterVec(prometheus.CounterOpts{
	Name: "twilight_kafka_refused_records_total",
	Help: "Records skipped because the database refused their data, by consumer.",
}, []string{"consumer"})

var consumerLag = promauto.NewGaugeVec(prometheus.GaugeOpts{
	Name: "twilight_kafka_consumer_lag",
	Help: "Records between a consumer group member's last poll and the end of the partitions it owns, by consumer.",
}, []string{"consumer"})

func ConsumeGroup(operation context.Context, options []kgo.Opt, name, group, topic string, logger *slog.Logger, handle func(context.Context, *kgo.Record) error) {
	backoff := time.Second
	for operation.Err() == nil {
		client, failure := kgo.NewClient(append(append([]kgo.Opt(nil), options...),
			kgo.ConsumerGroup(group),
			kgo.ConsumeTopics(topic),
			kgo.DisableAutoCommit(),
			kgo.ConsumeResetOffset(kgo.NewOffset().AtStart()),
			kgo.FetchMaxWait(time.Second),
			kgo.BlockRebalanceOnPoll(),
		)...)
		if failure != nil {
			logger.Error("kafka consumer could not be created", "consumer", name, "error", failure)
		} else {
			logger.Info("consumer started", "consumer", name, "group", group, "topic", topic)
			var uncommitted []*kgo.Record
			commitBackoff, commitDelay := commitRetryFirst, time.Duration(0)
			for operation.Err() == nil {
				polling, stop := operation, func() {}
				if len(uncommitted) > 0 {
					polling, stop = context.WithTimeout(operation, commitDelay)
				}
				fetches := client.PollFetches(polling)
				stop()
				if operation.Err() != nil {
					break
				}
				if failure := FetchFailure(fetches); failure != nil {
					logger.Warn("consumer fetch failed", "consumer", name, "error", failure)
					break
				}
				backoff = time.Second
				applied, stopped := applyPartitions(operation, name, logger, fetches, handle)
				pending, moved := stillUncommitted(append(uncommitted, applied...), client.CommittedOffsets())
				if len(moved) > 0 {
					logger.Warn("the offsets of records applied here were not committed before their partitions moved; their next owner applies them again",
						"consumer", name, "topic", topic, "partitions", moved)
				}
				uncommitted = nil
				if len(pending) > 0 {
					committing, cancel := context.WithTimeout(context.WithoutCancel(operation), 10*time.Second)
					failure := client.CommitRecords(committing, pending...)
					cancel()
					if failure != nil {
						uncommitted = pending
						commitDelay = time.Duration(rand.Int64N(int64(commitBackoff))) + 1
						commitBackoff = min(2*commitBackoff, commitRetryLimit)
						logger.Warn("consumer offsets were not committed; they are committed again with the next records or after a wait", "consumer", name,
							"partitions", len(pending), "retry_in_seconds", commitDelay.Seconds(), "error", failure)
					} else {
						commitBackoff = commitRetryFirst
					}
				}
				rewind(client, stopped)
				client.AllowRebalance()
			}
			if len(uncommitted) > 0 {
				logger.Warn("the consumer stopped before the offsets of records it applied were committed; their partitions' next owner applies them again",
					"consumer", name, "topic", topic, "partitions", len(uncommitted))
			}
			client.CloseAllowingRebalance()
			logger.Info("consumer stopped", "consumer", name, "group", group)
		}
		select {
		case <-operation.Done():
			return
		case <-time.After(time.Duration(rand.Int64N(int64(backoff))) + 1):
		}
		backoff = min(2*backoff, time.Minute)
	}
}

const (
	commitRetryFirst = 500 * time.Millisecond
	commitRetryLimit = 30 * time.Second
)

func stillUncommitted(records []*kgo.Record, committed map[string]map[int32]kgo.EpochOffset) ([]*kgo.Record, []int32) {
	latest := map[int32]*kgo.Record{}
	for _, record := range records {
		if previous := latest[record.Partition]; previous == nil || previous.Offset < record.Offset {
			latest[record.Partition] = record
		}
	}
	var kept []*kgo.Record
	var moved []int32
	for partition, record := range latest {
		offset, assigned := committed[record.Topic][partition]
		switch {
		case !assigned:
			moved = append(moved, partition)
		case offset.Offset <= record.Offset:
			kept = append(kept, record)
		}
	}
	slices.Sort(moved)
	return kept, moved
}

func rewind(client *kgo.Client, stopped []*kgo.Record) {
	if len(stopped) == 0 {
		return
	}
	offsets := map[string]map[int32]kgo.EpochOffset{}
	for _, record := range stopped {
		if offsets[record.Topic] == nil {
			offsets[record.Topic] = map[int32]kgo.EpochOffset{}
		}
		offsets[record.Topic][record.Partition] = kgo.EpochOffset{Epoch: record.LeaderEpoch, Offset: record.Offset}
	}
	client.SetOffsets(offsets)
}

func applyPartitions(operation context.Context, name string, logger *slog.Logger, fetches kgo.Fetches, handle func(context.Context, *kgo.Record) error) ([]*kgo.Record, []*kgo.Record) {
	var mutex sync.Mutex
	var applied, stopped []*kgo.Record
	var running sync.WaitGroup
	var lag int64
	fetches.EachPartition(func(partition kgo.FetchTopicPartition) {
		if len(partition.Records) == 0 {
			return
		}
		lag += max(0, partition.HighWatermark-partition.Records[len(partition.Records)-1].Offset-1)
		running.Add(1)
		go func() {
			defer running.Done()
			var last, first *kgo.Record
			for _, record := range partition.Records {
				done, stop := applyRecord(operation, name, logger, record, handle)
				if stop {
					first = record
				}
				if !done {
					break
				}
				last = record
			}
			mutex.Lock()
			if last != nil {
				applied = append(applied, last)
			}
			if first != nil {
				stopped = append(stopped, first)
			}
			mutex.Unlock()
		}()
	})
	running.Wait()
	consumerLag.WithLabelValues(name).Set(float64(lag))
	return applied, stopped
}

var ErrStopPartition = errors.New("the partition's records are not this member's to apply")

func applyRecord(operation context.Context, name string, logger *slog.Logger, record *kgo.Record, handle func(context.Context, *kgo.Record) error) (bool, bool) {
	backoff := time.Second
	for {
		failure := handle(operation, record)
		if failure == nil {
			return true, false
		}
		if operation.Err() != nil {
			return false, false
		}
		if errors.Is(failure, ErrStopPartition) {
			logger.Debug("a partition's records are left until it is resumed; it is paused at this record", "consumer", name, "topic", record.Topic,
				"partition", record.Partition, "offset", record.Offset, "error", failure)
			return false, true
		}
		var refused *pgconn.PgError
		if errors.As(failure, &refused) && strings.HasPrefix(refused.Code, "22") {
			refusedRecords.WithLabelValues(name).Inc()
			logger.Error("the database refused a record's data; it is skipped", "consumer", name, "topic", record.Topic,
				"partition", record.Partition, "offset", record.Offset, "sqlstate", refused.Code, "error", failure)
			return true, false
		}
		logger.Warn("a record was not applied; it is retried before anything after it in its partition", "consumer", name, "topic", record.Topic,
			"partition", record.Partition, "offset", record.Offset, "error", failure)
		select {
		case <-operation.Done():
			return false, false
		case <-time.After(time.Duration(rand.Int64N(int64(backoff))) + 1):
		}
		backoff = min(2*backoff, 30*time.Second)
	}
}
