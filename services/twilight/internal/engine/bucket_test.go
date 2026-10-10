package engine

import (
	"testing"
	"time"
)

func TestTokenBucketStartsEmptyAndRefillsToBurst(test *testing.T) {
	bucket := NewTokenBucket(2, 5, base)
	if bucket.Take(base) {
		test.Fatal("a new bucket must start empty")
	}
	if wait := bucket.Wait(base); wait != 500*time.Millisecond {
		test.Fatalf("wait %v", wait)
	}
	later := base.Add(time.Second)
	if !bucket.Take(later) || !bucket.Take(later) || bucket.Take(later) {
		test.Fatal("one second at 2/s must give exactly two tokens")
	}
	full := base.Add(time.Hour)
	taken := 0
	for bucket.Take(full) {
		taken++
	}
	if taken != 5 {
		test.Fatalf("burst gave %d tokens, want 5", taken)
	}
	bucket.Return(3)
	if !bucket.Available(full) {
		test.Fatal("returned tokens are not available")
	}
	bucket.Return(100)
	taken = 0
	for bucket.Take(full) {
		taken++
	}
	if taken != 5 {
		test.Fatalf("returning tokens exceeded the burst: %d", taken)
	}
	bucket.Reconfigure(1, 1)
	if !bucket.Take(full.Add(time.Second)) || bucket.Take(full.Add(time.Second)) {
		test.Fatal("reconfigured bucket")
	}
}
