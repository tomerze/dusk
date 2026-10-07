package reconcile

import (
	"crypto/ed25519"
	"encoding/base64"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"dusk/services/twilight/internal/kafka"
)

var start = time.Date(2026, 10, 6, 9, 15, 40, 512408217, time.UTC)

const pidFixture = "12808937078074471924"

func breakKinds(breaks []Break) string {
	var kinds []string
	for _, found := range breaks {
		kinds = append(kinds, found.Kind)
	}
	return strings.Join(kinds, ",")
}

func applyAll(test *testing.T, chains *Chains, entries ...kafka.LedgerEntry) string {
	test.Helper()
	var kinds []string
	for _, entry := range entries {
		breaks, _ := chains.Apply(chained(test, entry))
		if found := breakKinds(breaks); found != "" {
			kinds = append(kinds, found)
		}
	}
	return strings.Join(kinds, ";")
}

func cleanChain(writer *ledgerWriter, calls int) []kafka.LedgerEntry {
	var entries []kafka.LedgerEntry
	for range calls {
		entry, _ := writer.seal(writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
		entries = append(entries, entry)
	}
	checkpoint, _ := writer.checkpoint()
	return append(entries, checkpoint)
}

func TestAnIntactChainWithCheckpointsHasNoBreak(test *testing.T) {
	private, _ := signingKey(1)
	chains := NewChains(keySet(test, private), time.Second)
	writer := newWriter("nightfall-0", 0, start, private)
	var entries []kafka.LedgerEntry
	for range 5 {
		entries = append(entries, cleanChain(writer, 4)...)
	}
	if found := applyAll(test, chains, entries...); found != "" {
		test.Fatalf("breaks in an intact chain: %s", found)
	}
	head := chains.Heads[ChainKey{"nightfall-0", 0}]
	if head.Sequence != 24 || head.CheckpointSequence == nil || *head.CheckpointSequence != 24 || head.UnsignedSince != nil {
		test.Fatalf("head %+v", head)
	}
	tail := chains.Tails[0]
	if tail.Instance != "nightfall-0" || tail.Sequence != 24 || tail.Hash != head.Hash {
		test.Fatalf("tail %+v", tail)
	}
}

func TestAnEditedEntryIsAHashMismatch(test *testing.T) {
	private, _ := signingKey(1)
	chains := NewChains(keySet(test, private), time.Second)
	writer := newWriter("nightfall-0", 0, start, private)
	entries := cleanChain(writer, 4)
	entries[2].Principal = "mallory"
	if found := applyAll(test, chains, entries...); found != BreakHashMismatch {
		test.Fatalf("got %q", found)
	}
}

func TestADeletedEntryIsASequenceGap(test *testing.T) {
	private, _ := signingKey(1)
	chains := NewChains(keySet(test, private), time.Second)
	writer := newWriter("nightfall-0", 0, start, private)
	entries := cleanChain(writer, 4)
	entries = append(entries[:2], entries[3:]...)
	if found := applyAll(test, chains, entries...); found != BreakSequenceGap {
		test.Fatalf("got %q", found)
	}
}

func TestReorderedEntriesBreakTheChain(test *testing.T) {
	private, _ := signingKey(1)
	chains := NewChains(keySet(test, private), time.Second)
	writer := newWriter("nightfall-0", 0, start, private)
	entries := cleanChain(writer, 4)
	entries[1], entries[2] = entries[2], entries[1]
	if found := applyAll(test, chains, entries...); found != BreakSequenceGap+";"+BreakOutOfOrder {
		test.Fatalf("got %q", found)
	}
}

func TestADuplicateIsNotABreakButAConflictIs(test *testing.T) {
	private, _ := signingKey(1)
	chains := NewChains(keySet(test, private), time.Second)
	writer := newWriter("nightfall-0", 0, start, private)
	entries := cleanChain(writer, 4)
	if found := applyAll(test, chains, append(entries, entries[1], entries[4])...); found != "" {
		test.Fatalf("duplicates reported as breaks: %q", found)
	}
	if chains.Duplicates != 2 {
		test.Fatalf("duplicates counted %d", chains.Duplicates)
	}
	_, duplicate := chains.Apply(chained(test, entries[1]))
	if !duplicate {
		test.Fatal("a repeated entry was not a duplicate")
	}
	other := newWriter("nightfall-0", 0, start, private)
	first, _ := other.seal(other.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	second, _ := other.seal(other.call("dawn-1", pidFixture, fixtureSession, "Dusk.process", 3))
	if first.Hash != entries[0].Hash {
		test.Fatal("the fixture is not deterministic")
	}
	if found := applyAll(test, chains, second); found != BreakConflictingEntry {
		test.Fatalf("a different entry at a seen sequence: %q", found)
	}
}

func TestAPreviousHashThatDoesNotLinkIsABreak(test *testing.T) {
	private, _ := signingKey(1)
	chains := NewChains(keySet(test, private), time.Second)
	writer := newWriter("nightfall-0", 0, start, private)
	first, _ := writer.seal(writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	writer.previous = strings.Repeat("b", 64)
	second, _ := writer.seal(writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	if found := applyAll(test, chains, first, second); found != BreakPreviousHashMismatch {
		test.Fatalf("got %q", found)
	}
}

func TestForgedAndUnknownCheckpoints(test *testing.T) {
	private, _ := signingKey(1)
	stranger, _ := signingKey(2)
	cases := map[string]func(*ledgerWriter) kafka.LedgerEntry{
		BreakUnknownKey: func(writer *ledgerWriter) kafka.LedgerEntry {
			writer.signer, writer.keyID = stranger, Thumbprint(stranger.Public().(ed25519.PublicKey))
			entry, _ := writer.checkpoint()
			return entry
		},
		BreakForgedCheckpoint: func(writer *ledgerWriter) kafka.LedgerEntry {
			entry := writer.blank("checkpoint")
			message, _ := CheckpointMessage(writer.instance, writer.partition, writer.sequence-2, writer.previous)
			entry.KeyID = text(writer.keyID)
			entry.Signature = text(base64.StdEncoding.EncodeToString(ed25519.Sign(writer.signer, message)))
			sealed, _ := writer.seal(entry)
			return sealed
		},
		BreakForgedCheckpoint + " malformed": func(writer *ledgerWriter) kafka.LedgerEntry {
			entry := writer.blank("checkpoint")
			entry.KeyID = text(writer.keyID)
			entry.Signature = text("AAAA")
			sealed, _ := writer.seal(entry)
			return sealed
		},
	}
	for want, forge := range cases {
		chains := NewChains(keySet(test, private), time.Second)
		writer := newWriter("nightfall-0", 0, start, private)
		for range 3 {
			entry, _ := writer.seal(writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
			if found := applyAll(test, chains, entry); found != "" {
				test.Fatalf("%s: setup breaks %q", want, found)
			}
		}
		if found := applyAll(test, chains, forge(writer)); found != strings.Fields(want)[0] {
			test.Errorf("%s: got %q", want, found)
		}
	}
	chains := NewChains(keySet(test, private), time.Second)
	writer := newWriter("nightfall-0", 0, start, private)
	entry := writer.blank("checkpoint")
	entry.KeyID, entry.Signature = text(writer.keyID), text(base64.StdEncoding.EncodeToString(make([]byte, 64)))
	sealed, _ := writer.seal(entry)
	if found := applyAll(test, chains, sealed); found != BreakForgedCheckpoint {
		test.Errorf("a checkpoint starting a chain: %q", found)
	}
}

func TestChainStartsMustLinkThePartitionTail(test *testing.T) {
	private, _ := signingKey(1)
	keys := keySet(test, private)
	previous := newWriter("nightfall-old", 3, start, private)
	history := cleanChain(previous, 3)
	tail := history[len(history)-1]

	linkedChains := NewChains(keys, time.Second)
	applyAll(test, linkedChains, history...)
	linked := newWriter("nightfall-3", 3, start.Add(time.Minute), private)
	link, _ := linked.seal(linked.event("chain_link", map[string]any{"instance": tail.Instance, "sequence": tail.Sequence, "hash": tail.Hash}))
	next, _ := linked.seal(linked.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	if found := applyAll(test, linkedChains, link, next); found != "" {
		test.Fatalf("a linked chain start: %q", found)
	}

	wrongChains := NewChains(keys, time.Second)
	applyAll(test, wrongChains, history...)
	wrong := newWriter("nightfall-3", 3, start.Add(time.Minute), private)
	badLink, _ := wrong.seal(wrong.event("chain_link", map[string]any{"instance": tail.Instance, "sequence": tail.Sequence - 1, "hash": tail.Hash}))
	if found := applyAll(test, wrongChains, badLink); found != BreakChainLinkMismatch {
		test.Fatalf("a chain_link naming the wrong tail: %q", found)
	}

	unlinkedChains := NewChains(keys, time.Second)
	applyAll(test, unlinkedChains, history...)
	unlinked := newWriter("nightfall-3", 3, start.Add(time.Minute), private)
	first, _ := unlinked.seal(unlinked.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	if found := applyAll(test, unlinkedChains, first); found != BreakUnlinkedChainStart {
		test.Fatalf("an unlinked chain start: %q", found)
	}

	midstreamChains := NewChains(keys, time.Second)
	applyAll(test, midstreamChains, history...)
	midstream := newWriter("nightfall-3", 3, start.Add(time.Minute), private)
	midstream.sequence, midstream.previous = 40, strings.Repeat("c", 64)
	joined, _ := midstream.seal(midstream.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	if found := applyAll(test, midstreamChains, joined); found != BreakUnlinkedChainStart {
		test.Fatalf("another instance appearing mid-sequence: %q", found)
	}

	invalidChains := NewChains(keys, time.Second)
	invalid := newWriter("nightfall-0", 0, start, private)
	invalid.previous = strings.Repeat("d", 64)
	invalidStart, _ := invalid.seal(invalid.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	if found := applyAll(test, invalidChains, invalidStart); found != BreakInvalidChainStart {
		test.Fatalf("a chain start with a previous hash: %q", found)
	}

	observerChains := NewChains(keys, time.Second)
	if found := applyAll(test, observerChains, history[2:]...); found != "" {
		test.Fatalf("observing a chain from the middle: %q", found)
	}
}

func TestChainResumedMustNameThePrecedingEntry(test *testing.T) {
	private, _ := signingKey(1)
	chains := NewChains(keySet(test, private), time.Second)
	writer := newWriter("nightfall-0", 0, start, private)
	entries := cleanChain(writer, 2)
	resumed, _ := writer.seal(writer.event("chain_resumed", map[string]any{"sequence": writer.sequence - 1, "hash": writer.previous}))
	if found := applyAll(test, chains, append(entries, resumed)...); found != "" {
		test.Fatalf("a correct chain_resumed: %q", found)
	}
	wrong, _ := writer.seal(writer.event("chain_resumed", map[string]any{"sequence": 1, "hash": writer.previous}))
	if found := applyAll(test, chains, wrong); found != BreakChainResumedMismatch {
		test.Fatalf("a chain_resumed naming another entry: %q", found)
	}
}

func TestAnUnsignedTailIsMeasuredFromTheOldestUncoveredEntry(test *testing.T) {
	private, _ := signingKey(1)
	chains := NewChains(keySet(test, private), time.Second)
	writer := newWriter("nightfall-0", 0, start, private)
	var found []string
	for index := range 30 {
		entry, _ := writer.seal(writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
		if index%9 == 8 {
			checkpoint, _ := writer.checkpoint()
			found = append(found, applyAll(test, chains, entry, checkpoint))
			continue
		}
		found = append(found, applyAll(test, chains, entry))
	}
	checkpoint, _ := writer.checkpoint()
	found = append(found, applyAll(test, chains, checkpoint))
	if joined := strings.Join(found, ""); joined != "" {
		test.Fatalf("checkpoints within the interval: %q", joined)
	}

	sparse, _ := writer.seal(writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	writer.clock = writer.clock.Add(5 * time.Second)
	later, _ := writer.seal(writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	if result := applyAll(test, chains, sparse, later); result != BreakUnsignedTail {
		test.Fatalf("an entry five seconds after an unsigned one, with no checkpoint between: %q", result)
	}
	checkpoint, _ = writer.checkpoint()
	if result := applyAll(test, chains, checkpoint); result != "" {
		test.Fatalf("a checkpoint after the unsigned tail: %q", result)
	}

	beforeRestart, _ := writer.seal(writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	writer.clock = writer.clock.Add(10 * time.Minute)
	resumed, _ := writer.seal(writer.event("chain_resumed", map[string]any{"sequence": writer.sequence - 1, "hash": writer.previous}))
	checkpoint, _ = writer.checkpoint()
	if result := applyAll(test, chains, beforeRestart, resumed, checkpoint); result != "" {
		test.Fatalf("a chain resumed after a restart counted the time it was down: %q", result)
	}

	var unsigned []string
	for range 40 {
		entry, _ := writer.seal(writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
		if result := applyAll(test, chains, entry); result != "" {
			unsigned = append(unsigned, result)
		}
	}
	if len(unsigned) != 1 || unsigned[0] != BreakUnsignedTail {
		test.Fatalf("four seconds of entries without a checkpoint: %v", unsigned)
	}
}
func TestVerifyingKeysCheckTheirThumbprints(test *testing.T) {
	private, keyID := signingKey(3)
	public := private.Public().(ed25519.PublicKey)
	encoded := base64.RawURLEncoding.EncodeToString(public)
	keys, failure := ParseVerifyingKeys([]byte(`{"keys":[{"kty":"OKP","crv":"Ed25519","x":"` + encoded + `"}]}`))
	if failure != nil || keys.Count() != 1 {
		test.Fatalf("keys %v, error %v", keys, failure)
	}
	message := []byte("checkpoint")
	signature := base64.StdEncoding.EncodeToString(ed25519.Sign(private, message))
	if failure := keys.Verify(keyID, message, signature); failure != nil {
		test.Fatal(failure)
	}
	if failure := keys.Verify(keyID, []byte("other"), signature); failure != ErrInvalidSignature {
		test.Fatalf("a wrong message: %v", failure)
	}
	if failure := keys.Verify("nobody", message, signature); failure != ErrUnknownKey {
		test.Fatalf("an unknown key: %v", failure)
	}
	for _, document := range []string{
		`{"keys":[]}`,
		`{"keys":[{"kty":"EC","crv":"P-256","x":"` + encoded + `"}]}`,
		`{"keys":[{"kty":"OKP","crv":"Ed25519","x":"AAAA"}]}`,
		`{"keys":[{"kty":"OKP","crv":"Ed25519","x":"` + encoded + `","kid":"chosen-by-hand"}]}`,
		`not json`,
	} {
		if _, failure := ParseVerifyingKeys([]byte(document)); failure == nil {
			test.Errorf("accepted %s", document)
		}
	}
}

func TestThumbprintIsRFC7638(test *testing.T) {
	public, _ := base64.RawURLEncoding.DecodeString("11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo")
	if got := Thumbprint(public); got != "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k" {
		test.Fatalf("thumbprint %s", got)
	}
}

func TestLoadVerifyingKeysFromAFile(test *testing.T) {
	private, keyID := signingKey(9)
	public := base64.RawURLEncoding.EncodeToString(private.Public().(ed25519.PublicKey))
	path := filepath.Join(test.TempDir(), "ledger-keys.json")
	if failure := os.WriteFile(path, []byte(`{"keys":[{"kty":"OKP","crv":"Ed25519","x":"`+public+`","kid":"`+keyID+`"}]}`), 0o600); failure != nil {
		test.Fatal(failure)
	}
	if loaded, failure := LoadVerifyingKeys(path); failure != nil || loaded.Count() != 1 {
		test.Fatalf("loaded %v, error %v", loaded, failure)
	}
	if _, failure := LoadVerifyingKeys(filepath.Join(test.TempDir(), "absent.json")); failure == nil {
		test.Fatal("an absent key file was accepted")
	}
}
