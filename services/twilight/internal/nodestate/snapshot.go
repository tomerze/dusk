package nodestate

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/kafka"
)

const DefaultStallTimeout = 2 * time.Minute

var lifecycleNames = []string{"", "enrolled", "active", "quarantined", "retired", "revoked"}

func lifecycleCode(lifecycle string) uint8 {
	for code, name := range lifecycleNames {
		if name == lifecycle {
			return uint8(code)
		}
	}
	return 0
}

type Entry struct {
	lifecycle uint8
	visited   bool
	published bool
	partition int32
	time      int64
	reason    *string
	actor     string
}

func NewEntry(lifecycle string, at time.Time) *Entry {
	return &Entry{lifecycle: lifecycleCode(lifecycle), time: at.UnixNano()}
}

func (entry *Entry) Lifecycle() string {
	return lifecycleNames[entry.lifecycle]
}

func (entry *Entry) Time() time.Time {
	return time.Unix(0, entry.time).UTC()
}

func (entry *Entry) Reason() *string {
	return entry.reason
}

func (entry *Entry) Actor() string {
	return entry.actor
}

func (entry *Entry) Partition() int32 {
	return entry.partition
}

func (snapshot *Snapshot) keep(key string, entry *Entry) {
	if held, found := snapshot.States[key]; found && held.partition != entry.partition {
		if entry.time < held.time || entry.time == held.time && held.partition == partitionOf(key, snapshot.Partitions) {
			return
		}
	}
	snapshot.States[key] = entry
}

type Snapshot struct {
	Topic      string
	Partitions int32
	States     map[string]*Entry
	Complete   map[int32]bool
	interned   map[string]*string
}

func (snapshot Snapshot) Incomplete() []int32 {
	var missing []int32
	for partition := range snapshot.Partitions {
		if !snapshot.Complete[partition] {
			missing = append(missing, partition)
		}
	}
	return missing
}

func (snapshot *Snapshot) intern(text string) *string {
	if stored, found := snapshot.interned[text]; found {
		return stored
	}
	stored := &text
	snapshot.interned[text] = stored
	return stored
}

var (
	deviceKey       = regexp.MustCompile(`^device/[0-9a-f]{32}$`)
	installationKey = regexp.MustCompile(`^installation/[0-9a-f]{32}/[0-9a-f]{32}$`)
	completeKey     = regexp.MustCompile(`^complete/[0-9]+$`)
)

type value struct {
	Time      string  `json:"time"`
	Record    *string `json:"record"`
	Lifecycle string  `json:"lifecycle"`
	Reason    *string `json:"reason"`
	Actor     string  `json:"actor"`
}

func (snapshot *Snapshot) apply(record *kgo.Record, validator *kafka.Validator, logger *slog.Logger) {
	key := string(record.Key)
	switch {
	case completeKey.MatchString(key):
		if record.Value == nil {
			delete(snapshot.Complete, record.Partition)
			return
		}
		decoded, valid := kafka.Decode[value](validator, kafka.NodeStateContract(record.Value), record, logger)
		if valid && decoded.Record != nil {
			snapshot.Complete[record.Partition] = true
		}
	case deviceKey.MatchString(key), installationKey.MatchString(key):
		if snapshot.States == nil {
			return
		}
		if record.Value == nil {
			snapshot.keep(key, &Entry{time: record.Timestamp.UnixNano(), partition: record.Partition})
			return
		}
		decoded, valid := kafka.Decode[value](validator, kafka.NodeStateContract(record.Value), record, logger)
		if !valid || decoded.Record != nil {
			return
		}
		at, failure := kafka.ParseTime(decoded.Time)
		if failure != nil {
			at = record.Timestamp
		}
		entry := &Entry{lifecycle: lifecycleCode(decoded.Lifecycle), time: at.UnixNano(), partition: record.Partition, actor: *snapshot.intern(decoded.Actor)}
		if decoded.Reason != nil {
			entry.reason = snapshot.intern(*decoded.Reason)
		}
		snapshot.keep(key, entry)
	default:
		kafka.CountInvalid(record.Topic)
		logger.Warn("dropped a node-state record under a key no producer writes", "topic", record.Topic, "partition", record.Partition, "offset", record.Offset, "key", key)
	}
}

func partitionsOf(operation context.Context, admin *kadm.Client, topic string) (kadm.TopicDetail, error) {
	details, failure := admin.ListTopics(operation, topic)
	if failure != nil {
		return kadm.TopicDetail{}, fmt.Errorf("list %s: %w", topic, failure)
	}
	detail, found := details[topic]
	if !found || detail.Err != nil {
		return kadm.TopicDetail{}, fmt.Errorf("topic %s is not available: %v", topic, detail.Err)
	}
	return detail, nil
}

func Identity(operation context.Context, options []kgo.Opt, topic string) (string, error) {
	client, failure := kgo.NewClient(options...)
	if failure != nil {
		return "", failure
	}
	defer client.Close()
	detail, failure := partitionsOf(operation, kadm.NewClient(client), topic)
	if failure != nil {
		return "", failure
	}
	return fmt.Sprintf("%x/%d", detail.ID, len(detail.Partitions)), nil
}

var ErrUnread = errors.New("node-state partitions stopped short of their end offsets")

func Read(operation context.Context, options []kgo.Opt, topic string, validator *kafka.Validator, logger *slog.Logger, keepStates bool, stall time.Duration) (Snapshot, error) {
	if stall <= 0 {
		stall = DefaultStallTimeout
	}
	client, failure := kgo.NewClient(append(append([]kgo.Opt(nil), options...),
		kgo.FetchMaxWait(time.Second), kgo.FetchIsolationLevel(kgo.ReadCommitted()), kgo.KeepControlRecords())...)
	if failure != nil {
		return Snapshot{}, failure
	}
	defer client.Close()
	admin := kadm.NewClient(client)
	detail, failure := partitionsOf(operation, admin, topic)
	if failure != nil {
		return Snapshot{}, failure
	}
	starts, failure := admin.ListStartOffsets(operation, topic)
	if failure != nil {
		return Snapshot{}, fmt.Errorf("start offsets of %s: %w", topic, failure)
	}
	ends, failure := admin.ListEndOffsets(operation, topic)
	if failure != nil {
		return Snapshot{}, fmt.Errorf("end offsets of %s: %w", topic, failure)
	}
	partitions := detail.Partitions.Numbers()
	snapshot := Snapshot{Topic: topic, Partitions: int32(len(partitions)), Complete: map[int32]bool{}, interned: map[string]*string{}}
	if keepStates {
		snapshot.States = map[string]*Entry{}
	}
	assignment := map[int32]kgo.Offset{}
	remaining := map[int32]int64{}
	for _, partition := range partitions {
		assignment[partition] = kgo.NewOffset().AtStart()
		start, _ := starts.Lookup(topic, partition)
		end, found := ends.Lookup(topic, partition)
		if !found || end.Err != nil {
			return Snapshot{}, fmt.Errorf("end offset of %s[%d]: %v", topic, partition, end.Err)
		}
		if end.Offset > start.Offset {
			remaining[partition] = end.Offset
		}
	}
	client.AddConsumePartitions(map[string]map[int32]kgo.Offset{topic: assignment})
	progressed := map[int32]time.Time{}
	for partition := range remaining {
		progressed[partition] = time.Now()
	}
	for len(remaining) > 0 {
		polling, cancel := context.WithTimeout(operation, time.Second)
		fetches := client.PollFetches(polling)
		cancel()
		if operation.Err() != nil {
			return Snapshot{}, operation.Err()
		}
		if failure := kafka.FetchFailure(fetches); failure != nil {
			return Snapshot{}, failure
		}
		fetches.EachRecord(func(record *kgo.Record) {
			if !record.Attrs.IsControl() {
				snapshot.apply(record, validator, logger)
			}
			end, waiting := remaining[record.Partition]
			if !waiting {
				return
			}
			progressed[record.Partition] = time.Now()
			if record.Offset+1 >= end {
				delete(remaining, record.Partition)
			}
		})
		var stalled []string
		for partition, end := range remaining {
			if time.Since(progressed[partition]) > stall {
				stalled = append(stalled, fmt.Sprintf("%d (end offset %d)", partition, end))
			}
		}
		if len(stalled) > 0 {
			sort.Strings(stalled)
			return Snapshot{}, fmt.Errorf("%w: %s made no progress for %s: %s", ErrUnread, topic, stall, strings.Join(stalled, ", "))
		}
	}
	return snapshot, nil
}
