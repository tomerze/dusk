package reconcile

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"dusk/services/twilight/internal/kafka"
)

var chainStartHash = strings.Repeat("0", 64)

const (
	BreakHashMismatch         = "hash_mismatch"
	BreakPreviousHashMismatch = "previous_hash_mismatch"
	BreakSequenceGap          = "sequence_gap"
	BreakOutOfOrder           = "out_of_order"
	BreakConflictingEntry     = "conflicting_entry"
	BreakStaleEntry           = "stale_entry"
	BreakInvalidChainStart    = "invalid_chain_start"
	BreakUnlinkedChainStart   = "unlinked_chain_start"
	BreakChainLinkMismatch    = "chain_link_mismatch"
	BreakChainResumedMismatch = "chain_resumed_mismatch"
	BreakUnknownKey           = "unknown_key"
	BreakForgedCheckpoint     = "forged_checkpoint"
	BreakUnsignedTail         = "unsigned_tail"
	BreakWrongPartition       = "wrong_partition"
)

const recentWindow = 1024

type ChainEntry struct {
	Kind         string
	Instance     string
	Partition    int64
	Sequence     int64
	PreviousHash string
	Hash         string
	ComputedHash string
	Time         time.Time
	Event        string
	EventDetail  map[string]any
	KeyID        string
	Signature    string
}

func ChainEntryOf(entry kafka.LedgerEntry, computedHash string) (ChainEntry, error) {
	at, failure := kafka.ParseTime(entry.Time)
	if failure != nil {
		return ChainEntry{}, fmt.Errorf("entry time: %w", failure)
	}
	chained := ChainEntry{
		Kind: entry.Kind, Instance: entry.Instance, Partition: entry.Partition, Sequence: entry.Sequence,
		PreviousHash: entry.PreviousHash, Hash: entry.Hash, ComputedHash: computedHash, Time: at,
	}
	if entry.Event != nil {
		chained.Event = *entry.Event
	}
	if entry.KeyID != nil {
		chained.KeyID = *entry.KeyID
	}
	if entry.Signature != nil {
		chained.Signature = *entry.Signature
	}
	if len(entry.EventDetail) > 0 && !bytes.Equal(entry.EventDetail, []byte("null")) {
		decoder := json.NewDecoder(bytes.NewReader(entry.EventDetail))
		decoder.UseNumber()
		if failure := decoder.Decode(&chained.EventDetail); failure != nil {
			return ChainEntry{}, fmt.Errorf("event_detail: %w", failure)
		}
	}
	return chained, nil
}

type ChainKey struct {
	Instance  string
	Partition int64
}

type sequenceHash struct {
	sequence int64
	hash     string
}

type ChainHead struct {
	Sequence           int64
	Hash               string
	CheckpointSequence *int64
	UnsignedSince      *time.Time
	recent             []sequenceHash
	next               int
}

func (head *ChainHead) remember(sequence int64, hash string) {
	if len(head.recent) < recentWindow {
		head.recent = append(head.recent, sequenceHash{sequence, hash})
		return
	}
	head.recent[head.next] = sequenceHash{sequence, hash}
	head.next = (head.next + 1) % recentWindow
}

func (head *ChainHead) lookup(sequence int64) (string, bool) {
	if sequence == head.Sequence {
		return head.Hash, true
	}
	for _, seen := range head.recent {
		if seen.sequence == sequence {
			return seen.hash, true
		}
	}
	return "", false
}

func (head *ChainHead) oldest() int64 {
	oldest := head.Sequence
	for _, seen := range head.recent {
		oldest = min(oldest, seen.sequence)
	}
	return oldest
}

type PartitionTail struct {
	Instance string
	Sequence int64
	Hash     string
}

type Break struct {
	Kind      string
	Instance  string
	Partition int64
	Sequence  int64
	Detail    string
}

type Chains struct {
	keys         *VerifyingKeys
	interval     time.Duration
	Heads        map[ChainKey]*ChainHead
	Tails        map[int64]*PartitionTail
	ChangedHeads map[ChainKey]bool
	ChangedTails map[int64]bool
	Duplicates   int64
}

func NewChains(keys *VerifyingKeys, checkpointInterval time.Duration) *Chains {
	return &Chains{
		keys: keys, interval: checkpointInterval,
		Heads: map[ChainKey]*ChainHead{}, Tails: map[int64]*PartitionTail{},
		ChangedHeads: map[ChainKey]bool{}, ChangedTails: map[int64]bool{},
	}
}

func (chains *Chains) ClearChanges() {
	clear(chains.ChangedHeads)
	clear(chains.ChangedTails)
}

func detailMatches(detail map[string]any, sequence int64, hash string) bool {
	number, isNumber := detail["sequence"].(json.Number)
	named, isText := detail["hash"].(string)
	return isNumber && isText && number.String() == fmt.Sprint(sequence) && named == hash
}

func (chains *Chains) Apply(entry ChainEntry) ([]Break, bool) {
	var breaks []Break
	report := func(kind, format string, arguments ...any) {
		breaks = append(breaks, Break{Kind: kind, Instance: entry.Instance, Partition: entry.Partition, Sequence: entry.Sequence, Detail: fmt.Sprintf(format, arguments...)})
	}
	key := ChainKey{Instance: entry.Instance, Partition: entry.Partition}
	head := chains.Heads[key]
	intact := entry.ComputedHash == entry.Hash
	if !intact {
		report(BreakHashMismatch, "stored hash %s but the content hashes to %s", entry.Hash, entry.ComputedHash)
	}
	if intact && head != nil && entry.Sequence <= head.Sequence {
		if seen, found := head.lookup(entry.Sequence); found && seen == entry.Hash {
			chains.Duplicates++
			return nil, true
		}
	}
	tail := chains.Tails[entry.Partition]
	switch {
	case entry.Sequence == 0:
		if entry.PreviousHash != chainStartHash {
			report(BreakInvalidChainStart, "a chain start carries previous_hash %s instead of 64 zeros", entry.PreviousHash)
		}
		if tail != nil {
			if entry.Event != "chain_link" {
				report(BreakUnlinkedChainStart, "a chain starts after instance %s sequence %d on this partition without a chain_link", tail.Instance, tail.Sequence)
			} else if named, _ := entry.EventDetail["instance"].(string); named != tail.Instance || !detailMatches(entry.EventDetail, tail.Sequence, tail.Hash) {
				report(BreakChainLinkMismatch, "chain_link names %v but the partition's previous entry is instance %s sequence %d hash %s", entry.EventDetail, tail.Instance, tail.Sequence, tail.Hash)
			}
		}
		head = &ChainHead{}
	case head == nil:
		if tail != nil && tail.Instance != entry.Instance {
			report(BreakUnlinkedChainStart, "the first entry of this chain follows instance %s sequence %d on the partition without a chain_link", tail.Instance, tail.Sequence)
		}
		head = &ChainHead{}
	default:
		if entry.Sequence <= head.Sequence {
			if seen, found := head.lookup(entry.Sequence); found {
				report(BreakConflictingEntry, "sequence %d already holds an entry with hash %s, this one has %s", entry.Sequence, seen, entry.Hash)
			} else if entry.Sequence < head.oldest() {
				report(BreakStaleEntry, "older than the %d entries kept for duplicate detection", recentWindow)
			} else {
				report(BreakOutOfOrder, "arrived after sequence %d", head.Sequence)
			}
			return breaks, false
		}
		if entry.Sequence > head.Sequence+1 {
			report(BreakSequenceGap, "sequences %d to %d are missing", head.Sequence+1, entry.Sequence-1)
		} else if entry.PreviousHash != head.Hash {
			report(BreakPreviousHashMismatch, "previous_hash %s but the preceding entry has hash %s", entry.PreviousHash, head.Hash)
		}
		if entry.Event == "chain_resumed" && !detailMatches(entry.EventDetail, entry.Sequence-1, entry.PreviousHash) {
			report(BreakChainResumedMismatch, "chain_resumed names %v but follows sequence %d hash %s", entry.EventDetail, entry.Sequence-1, entry.PreviousHash)
		}
	}
	if entry.Kind == "checkpoint" {
		chains.verifyCheckpoint(entry, report)
		sequence := entry.Sequence
		head.CheckpointSequence = &sequence
		head.UnsignedSince = nil
	} else {
		at := entry.Time
		switch {
		case head.UnsignedSince == nil || entry.Event == "chain_resumed":
			head.UnsignedSince = &at
		case entry.Time.Sub(*head.UnsignedSince) > 2*chains.interval:
			report(BreakUnsignedTail, "the entry of %s was still not covered by a checkpoint at %s, more than twice the checkpoint interval of %s later",
				head.UnsignedSince.UTC().Format(time.RFC3339Nano), entry.Time.UTC().Format(time.RFC3339Nano), chains.interval)
			head.UnsignedSince = &at
		}
	}
	head.Sequence = entry.Sequence
	head.Hash = entry.Hash
	head.remember(entry.Sequence, entry.Hash)
	chains.Heads[key] = head
	chains.ChangedHeads[key] = true
	chains.Tails[entry.Partition] = &PartitionTail{Instance: entry.Instance, Sequence: entry.Sequence, Hash: entry.Hash}
	chains.ChangedTails[entry.Partition] = true
	return breaks, false
}

func (chains *Chains) verifyCheckpoint(entry ChainEntry, report func(kind, format string, arguments ...any)) {
	if entry.Sequence == 0 {
		report(BreakForgedCheckpoint, "a checkpoint cannot start a chain")
		return
	}
	if entry.KeyID == "" || entry.Signature == "" {
		report(BreakForgedCheckpoint, "a checkpoint without key_id or signature")
		return
	}
	message, failure := CheckpointMessage(entry.Instance, entry.Partition, entry.Sequence-1, entry.PreviousHash)
	if failure == nil {
		failure = chains.keys.Verify(entry.KeyID, message, entry.Signature)
	}
	switch {
	case failure == nil:
	case errors.Is(failure, ErrUnknownKey):
		report(BreakUnknownKey, "no configured ledger key has key id %s", entry.KeyID)
	default:
		report(BreakForgedCheckpoint, "the checkpoint signature over sequence %d does not verify: %v", entry.Sequence-1, failure)
	}
}
