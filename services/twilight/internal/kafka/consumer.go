package kafka

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"math/rand/v2"
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
			for operation.Err() == nil {
				fetches := client.PollFetches(operation)
				if operation.Err() != nil {
					break
				}
				if failure := FetchFailure(fetches); failure != nil {
					logger.Warn("consumer fetch failed", "consumer", name, "error", failure)
					break
				}
				backoff = time.Second
				applied := applyPartitions(operation, name, logger, fetches, handle)
				if len(applied) > 0 {
					committing, cancel := context.WithTimeout(context.WithoutCancel(operation), 10*time.Second)
					if failure := client.CommitRecords(committing, applied...); failure != nil {
						logger.Warn("consumer offsets were not committed; records may be applied again", "consumer", name, "error", failure)
					}
					cancel()
				}
				client.AllowRebalance()
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

func applyPartitions(operation context.Context, name string, logger *slog.Logger, fetches kgo.Fetches, handle func(context.Context, *kgo.Record) error) []*kgo.Record {
	var mutex sync.Mutex
	var applied []*kgo.Record
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
			var last *kgo.Record
			for _, record := range partition.Records {
				if !applyRecord(operation, name, logger, record, handle) {
					break
				}
				last = record
			}
			if last != nil {
				mutex.Lock()
				applied = append(applied, last)
				mutex.Unlock()
			}
		}()
	})
	running.Wait()
	consumerLag.WithLabelValues(name).Set(float64(lag))
	return applied
}

func applyRecord(operation context.Context, name string, logger *slog.Logger, record *kgo.Record, handle func(context.Context, *kgo.Record) error) bool {
	backoff := time.Second
	for {
		failure := handle(operation, record)
		if failure == nil {
			return true
		}
		if operation.Err() != nil {
			return false
		}
		var refused *pgconn.PgError
		if errors.As(failure, &refused) && strings.HasPrefix(refused.Code, "22") {
			refusedRecords.WithLabelValues(name).Inc()
			logger.Error("the database refused a record's data; it is skipped", "consumer", name, "topic", record.Topic,
				"partition", record.Partition, "offset", record.Offset, "sqlstate", refused.Code, "error", failure)
			return true
		}
		logger.Warn("a record was not applied; it is retried before anything after it in its partition", "consumer", name, "topic", record.Topic,
			"partition", record.Partition, "offset", record.Offset, "error", failure)
		select {
		case <-operation.Done():
			return false
		case <-time.After(time.Duration(rand.Int64N(int64(backoff))) + 1):
		}
		backoff = min(2*backoff, 30*time.Second)
	}
}
