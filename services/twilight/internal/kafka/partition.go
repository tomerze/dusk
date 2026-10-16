package kafka

func Murmur2(data []byte) int32 {
	const (
		seed       uint32 = 0x9747b28c
		multiplier uint32 = 0x5bd1e995
		shift             = 24
	)
	hash := seed ^ uint32(len(data))
	remaining := data
	for len(remaining) >= 4 {
		word := uint32(remaining[0]) | uint32(remaining[1])<<8 | uint32(remaining[2])<<16 | uint32(remaining[3])<<24
		word *= multiplier
		word ^= word >> shift
		word *= multiplier
		hash *= multiplier
		hash ^= word
		remaining = remaining[4:]
	}
	switch len(remaining) {
	case 3:
		hash ^= uint32(remaining[2]) << 16
		fallthrough
	case 2:
		hash ^= uint32(remaining[1]) << 8
		fallthrough
	case 1:
		hash ^= uint32(remaining[0])
		hash *= multiplier
	}
	hash ^= hash >> 13
	hash *= multiplier
	hash ^= hash >> 15
	return int32(hash)
}

func KeyPartition(key string, partitions int32) int32 {
	return (Murmur2([]byte(key)) & 0x7fffffff) % partitions
}
