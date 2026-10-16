package kafka

import (
	"fmt"
	"math/rand/v2"
	"testing"

	"github.com/twmb/franz-go/pkg/kgo"
)

func TestMurmur2MatchesKafka(test *testing.T) {
	for key, want := range map[string]int32{
		"21":                         -973932308,
		"foobar":                     -790332482,
		"a-little-bit-long-string":   -985981536,
		"a-little-bit-longer-string": -1486304829,
		"lkjh234lh9fiuh90y23oiuhsafujhadof229phr9h19h89h8": -58897971,
		"abc": 479470107,
	} {
		if got := Murmur2([]byte(key)); got != want {
			test.Errorf("murmur2(%q) = %d, Kafka's is %d", key, got, want)
		}
	}
}

func TestKeyPartitionMatchesKafkasDefaultPartitioner(test *testing.T) {
	for _, golden := range []struct {
		key        string
		partitions int32
		want       int32
	}{
		{"21", 3, 0}, {"21", 48, 12}, {"21", 128, 108},
		{"foobar", 48, 30}, {"a-little-bit-long-string", 48, 32}, {"a-little-bit-longer-string", 48, 35},
		{"lkjh234lh9fiuh90y23oiuhsafujhadof229phr9h19h89h8", 48, 29}, {"abc", 48, 27},
		{"3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13/a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70", 3, 1},
		{"3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13/a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70", 48, 28},
		{"3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13/a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70", 128, 28},
		{"000000000000000000000000000d0000/00000000000000000000000000010000", 3, 1},
		{"000000000000000000000000000d0000/00000000000000000000000000010000", 48, 31},
		{"000000000000000000000000000d0000/00000000000000000000000000010000", 128, 111},
		{"ffffffffffffffffffffffffffffffff/ffffffffffffffffffffffffffffffff", 3, 2},
		{"ffffffffffffffffffffffffffffffff/ffffffffffffffffffffffffffffffff", 48, 2},
		{"ffffffffffffffffffffffffffffffff/ffffffffffffffffffffffffffffffff", 128, 66},
	} {
		if got := KeyPartition(golden.key, golden.partitions); got != golden.want {
			test.Errorf("the partition of %q among %d is %d, Kafka's default partitioner picks %d", golden.key, golden.partitions, got, golden.want)
		}
	}
}

func TestKeyPartitionMatchesFranzGo(test *testing.T) {
	random := rand.New(rand.NewPCG(1, 2))
	partitioner := kgo.StickyKeyPartitioner(nil).ForTopic("dusk.process-results")
	for _, partitions := range []int32{1, 3, 6, 12, 48, 96, 128, 32767} {
		for range 2000 {
			length := random.IntN(80)
			key := make([]byte, length)
			for index := range key {
				key[index] = byte(random.UintN(256))
			}
			want := int32(partitioner.Partition(&kgo.Record{Key: key}, int(partitions)))
			if got := KeyPartition(string(key), partitions); got != want {
				test.Fatalf("the partition of %x among %d is %d, franz-go's key partitioner picks %d", key, partitions, got, want)
			}
		}
	}
	for index := range 2000 {
		key := fmt.Sprintf("%032x/%032x", random.Uint64(), uint64(index))
		want := int32(partitioner.Partition(&kgo.Record{Key: []byte(key)}, 48))
		if got := KeyPartition(key, 48); got != want {
			test.Fatalf("the partition of node key %s among 48 is %d, franz-go's key partitioner picks %d", key, got, want)
		}
	}
}
