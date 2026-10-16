package reconcile

import "testing"

func TestAPartitionIsLostWhenItEndsBeforeTheStoredOffsetOrItsTopicIsAnother(test *testing.T) {
	const first, second = "6f1c0d9a2b7e4c1f8a3d5e7b9c0f2a4d", "0a9b8c7d6e5f40312b3c4d5e6f708192"
	for _, check := range []struct {
		name         string
		storedOffset int64
		endOffset    int64
		storedTopic  string
		topic        string
		want         bool
	}{
		{"nothing stored", -1, 0, first, second, false},
		{"the partition went on", 41, 90, first, first, false},
		{"the partition ends before the stored offset", 41, 12, first, first, true},
		{"the partition ends at the stored offset", 41, 41, "", "", true},
		{"the topic was created again and grew past the stored offset", 41, 90, first, second, true},
		{"no topic id was stored yet", 41, 90, "", second, false},
		{"the broker reports no topic id", 41, 90, first, "", false},
		{"the end offset is unknown", 41, -1, first, first, false},
	} {
		if got := partitionLost(check.storedOffset, check.endOffset, check.storedTopic, check.topic); got != check.want {
			test.Errorf("%s: %v, want %v", check.name, got, check.want)
		}
	}
}
