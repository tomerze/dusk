package engine

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"testing"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/kafka"
)

func metricValue(test *testing.T, name string, labels map[string]string) float64 {
	test.Helper()
	families, failure := prometheus.DefaultGatherer.Gather()
	if failure != nil {
		test.Fatal(failure)
	}
	for _, family := range families {
		if family.GetName() != name {
			continue
		}
		for _, metric := range family.GetMetric() {
			matched := 0
			for _, pair := range metric.GetLabel() {
				if labels[pair.GetName()] == pair.GetValue() {
					matched++
				}
			}
			if matched == len(labels) {
				return metric.GetCounter().GetValue() + metric.GetGauge().GetValue()
			}
		}
	}
	return 0
}

func leaseLosses(test *testing.T, how string) float64 {
	return metricValue(test, "twilight_slice_lease_losses_total", map[string]string{"how": how})
}

func TestALostSliceIsDroppedAtOnceAndARevokedOneWaitsForItsWrites(test *testing.T) {
	owner := New(Dependencies{Config: config.Default(), Logger: slog.New(slog.NewJSONHandler(io.Discard, nil))})
	owner.ownership.partitions = 3
	keys := map[int32]NodeKey{}
	for index := 0; len(keys) < 3; index++ {
		key := NodeKey{DeviceID: fmt.Sprintf("%032x", index), InstallationID: fmt.Sprintf("%032x", 7)}
		keys[kafka.KeyPartition(key.String(), 3)] = key
	}
	for partition := range int32(3) {
		owner.ownership.own(campaign.Lease{Partition: partition, Epoch: 7})
	}
	client, failure := kgo.NewClient(kgo.SeedBrokers("127.0.0.1:1"))
	if failure != nil {
		test.Fatal(failure)
	}
	defer client.Close()
	topic := owner.Config.Kafka.Topics.ProcessResults
	finished := func(callback func(context.Context, *kgo.Client, map[string][]int32), partition int32) chan struct{} {
		done := make(chan struct{})
		go func() {
			callback(context.Background(), client, map[string][]int32{topic: {partition}})
			close(done)
		}()
		return done
	}

	lostBefore := leaseLosses(test, "lost")
	_, release, owned := owner.ownership.hold(keys[0])
	if !owned {
		test.Fatal("slice 0 is not owned")
	}
	select {
	case <-finished(owner.ownership.lost, 0):
	case <-time.After(5 * time.Second):
		test.Fatal("a lost slice waited for the writes under it")
	}
	release()
	if owner.ownership.owns(keys[0]) || leaseLosses(test, "lost") != lostBefore+1 {
		test.Fatalf("after losing slice 0: owned %v, lost slices counted %v", owner.ownership.owns(keys[0]), leaseLosses(test, "lost")-lostBefore)
	}

	_, release, owned = owner.ownership.hold(keys[1])
	if !owned {
		test.Fatal("slice 1 is not owned")
	}
	revoked := finished(owner.ownership.revoked, 1)
	select {
	case <-revoked:
		test.Fatal("a revoked slice was given up while a write under it was running")
	case <-time.After(300 * time.Millisecond):
	}
	release()
	select {
	case <-revoked:
	case <-time.After(5 * time.Second):
		test.Fatal("a revoked slice was not given up once its write finished")
	}
	if owner.ownership.owns(keys[1]) || leaseLosses(test, "lost") != lostBefore+1 {
		test.Fatal("a revoked slice is still owned, or was counted as lost")
	}

	owner.ownership.mutex.Lock()
	waiting := &pendingLease{since: time.Now(), retrying: true}
	owner.ownership.pending[2] = waiting
	owner.ownership.mutex.Unlock()
	revoked = finished(owner.ownership.revoked, 2)
	select {
	case <-revoked:
		test.Fatal("a slice was given up while its lease was being taken")
	case <-time.After(300 * time.Millisecond):
	}
	owner.ownership.mutex.Lock()
	waiting.retrying = false
	owner.ownership.settled.Broadcast()
	owner.ownership.mutex.Unlock()
	select {
	case <-revoked:
	case <-time.After(5 * time.Second):
		test.Fatal("a slice was not given up once the attempt to take its lease ended")
	}
	owner.ownership.mutex.Lock()
	_, stillPending := owner.ownership.pending[2]
	owner.ownership.mutex.Unlock()
	if stillPending || owner.ownership.owns(keys[2]) || owner.ownership.owning() {
		test.Fatal("a revoked slice is still pending or owned")
	}
}

func TestARevocationWaitsOnlyForItsOwnLeaseAttemptAndALostSliceCancelsIts(test *testing.T) {
	owner := New(Dependencies{Config: config.Default(), Logger: slog.New(slog.NewJSONHandler(io.Discard, nil))})
	owner.ownership.partitions = 3
	client, failure := kgo.NewClient(kgo.SeedBrokers("127.0.0.1:1"))
	if failure != nil {
		test.Fatal(failure)
	}
	defer client.Close()
	topic := owner.Config.Kafka.Topics.ProcessResults
	attempt, cancel := context.WithCancel(context.Background())
	defer cancel()
	running := &pendingLease{since: time.Now(), retrying: true, cancel: cancel}
	owner.ownership.mutex.Lock()
	owner.ownership.pending[0] = running
	owner.ownership.pending[1] = &pendingLease{since: time.Now()}
	owner.ownership.mutex.Unlock()
	go func() {
		<-attempt.Done()
		owner.ownership.mutex.Lock()
		running.retrying, running.cancel = false, nil
		owner.ownership.settled.Broadcast()
		owner.ownership.mutex.Unlock()
	}()

	revoked := make(chan struct{})
	go func() {
		owner.ownership.revoked(context.Background(), client, map[string][]int32{topic: {1}})
		close(revoked)
	}()
	select {
	case <-revoked:
	case <-time.After(5 * time.Second):
		test.Fatal("revoking a slice waited for the lease attempt of another slice")
	}
	if attempt.Err() != nil {
		test.Fatal("revoking one slice cancelled another slice's lease attempt")
	}

	lost := make(chan struct{})
	go func() {
		owner.ownership.lost(context.Background(), client, map[string][]int32{topic: {0}})
		close(lost)
	}()
	select {
	case <-lost:
	case <-time.After(5 * time.Second):
		test.Fatal("a lost slice's lease attempt was not cancelled, or losing it did not wait for the attempt to end")
	}
	if attempt.Err() == nil {
		test.Fatal("losing a slice left its lease attempt running")
	}
	owner.ownership.mutex.Lock()
	remaining := len(owner.ownership.pending)
	owner.ownership.mutex.Unlock()
	if remaining != 0 {
		test.Fatalf("%d slices are still pending after both were given up", remaining)
	}
}
