package reconcile

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"testing"
	"time"

	"dusk/services/twilight/internal/kafka"
)

const (
	fixtureDevice       = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13"
	fixtureInstallation = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
	fixtureNamespace    = "5d2e9a1c7b3f8e04"
	fixtureEpoch        = int64(1791278043512408)
	fixtureSession      = "0192f3a4-9e8d-7c6b-8a59-483726150f1e"
)

type ledgerWriter struct {
	instance  string
	partition int64
	sequence  int64
	previous  string
	clock     time.Time
	signer    ed25519.PrivateKey
	keyID     string
	counter   int
}

func signingKey(seed byte) (ed25519.PrivateKey, string) {
	material := sha256.Sum256([]byte{seed})
	private := ed25519.NewKeyFromSeed(material[:])
	return private, Thumbprint(private.Public().(ed25519.PublicKey))
}

func keySet(test testing.TB, privates ...ed25519.PrivateKey) *VerifyingKeys {
	test.Helper()
	var members []map[string]string
	for _, private := range privates {
		public := private.Public().(ed25519.PublicKey)
		members = append(members, map[string]string{"kty": "OKP", "crv": "Ed25519", "x": base64.RawURLEncoding.EncodeToString(public), "kid": Thumbprint(public)})
	}
	document, _ := json.Marshal(map[string]any{"keys": members})
	keys, failure := ParseVerifyingKeys(document)
	if failure != nil {
		test.Fatal(failure)
	}
	return keys
}

func newWriter(instance string, partition int64, start time.Time, signer ed25519.PrivateKey) *ledgerWriter {
	writer := &ledgerWriter{instance: instance, partition: partition, previous: chainStartHash, clock: start, signer: signer}
	if signer != nil {
		writer.keyID = Thumbprint(signer.Public().(ed25519.PublicKey))
	}
	return writer
}

func text(value string) *string { return &value }

func number(value int64) *int64 { return &value }

func (writer *ledgerWriter) identifier() string {
	writer.counter++
	return fmt.Sprintf("01a112fe-d1ba-7684-ae83-%012x", writer.counter)
}

func (writer *ledgerWriter) blank(kind string) kafka.LedgerEntry {
	return kafka.LedgerEntry{
		Envelope:        kafka.Envelope{Schema: "dusk.ledger/v1", ID: writer.identifier(), Time: kafka.FormatTime(writer.clock)},
		Kind:            kind,
		Instance:        writer.instance,
		Partition:       writer.partition,
		Principal:       "nightfall",
		Pid:             "0",
		ParameterFields: []kafka.ParameterField{},
		ParameterCapIDs: []int64{},
		ResultCapIDs:    []int64{},
	}
}

func (writer *ledgerWriter) call(principal, pid, session, action string, capID int64) kafka.LedgerEntry {
	entry := writer.blank("call")
	entry.DeviceID, entry.InstallationID, entry.NamespaceID, entry.Epoch = text(fixtureDevice), text(fixtureInstallation), text(fixtureNamespace), number(fixtureEpoch)
	entry.Principal, entry.Pid = principal, pid
	entry.SessionID, entry.CallID = text(session), text(writer.identifier())
	entry.CapID, entry.ParentCapID = number(capID), number(0)
	entry.Direction, entry.Action = text("client_to_node"), text(action)
	entry.InterfaceID, entry.MethodID = text("ace6963097d486d7"), number(0)
	entry.ParameterHash = "1f3e5d7c9b0a2f4e6d8c1b3a5f7e9d0c2b4a6f8e1d3c5b7a9f0e2d4c6b8a1f3e"
	return entry
}

func (writer *ledgerWriter) event(event string, detail map[string]any) kafka.LedgerEntry {
	entry := writer.blank("event")
	entry.Event = text(event)
	if detail != nil {
		entry.EventDetail, _ = json.Marshal(detail)
	}
	return entry
}

func (writer *ledgerWriter) seal(entry kafka.LedgerEntry) (kafka.LedgerEntry, []byte) {
	entry.Sequence = writer.sequence
	entry.PreviousHash = writer.previous
	entry.Hash = ""
	unsealed, _ := json.Marshal(entry)
	hash, failure := EntryHash(unsealed)
	if failure != nil {
		panic(failure)
	}
	entry.Hash = hash
	writer.sequence++
	writer.previous = hash
	writer.clock = writer.clock.Add(100 * time.Millisecond)
	sealed, _ := json.Marshal(entry)
	return entry, sealed
}

func (writer *ledgerWriter) checkpoint() (kafka.LedgerEntry, []byte) {
	entry := writer.blank("checkpoint")
	message, _ := CheckpointMessage(writer.instance, writer.partition, writer.sequence-1, writer.previous)
	entry.KeyID = text(writer.keyID)
	entry.Signature = text(base64.StdEncoding.EncodeToString(ed25519.Sign(writer.signer, message)))
	return writer.seal(entry)
}

func chained(test testing.TB, entry kafka.LedgerEntry) ChainEntry {
	test.Helper()
	encoded, _ := json.Marshal(entry)
	hash, failure := EntryHash(encoded)
	if failure != nil {
		test.Fatal(failure)
	}
	result, failure := ChainEntryOf(entry, hash)
	if failure != nil {
		test.Fatal(failure)
	}
	return result
}
