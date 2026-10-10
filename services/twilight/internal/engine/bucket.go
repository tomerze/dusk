package engine

import "time"

type TokenBucket struct {
	rate    float64
	burst   float64
	tokens  float64
	updated time.Time
}

func NewTokenBucket(rate float64, burst int, now time.Time) *TokenBucket {
	return &TokenBucket{rate: rate, burst: float64(burst), updated: now}
}

func (bucket *TokenBucket) refill(now time.Time) {
	if now.After(bucket.updated) {
		bucket.tokens = min(bucket.burst, bucket.tokens+now.Sub(bucket.updated).Seconds()*bucket.rate)
		bucket.updated = now
	}
}

func (bucket *TokenBucket) Take(now time.Time) bool {
	bucket.refill(now)
	if bucket.tokens < 1 {
		return false
	}
	bucket.tokens--
	return true
}

func (bucket *TokenBucket) Return(count int) {
	bucket.tokens = min(bucket.burst, bucket.tokens+float64(count))
}

func (bucket *TokenBucket) Available(now time.Time) bool {
	bucket.refill(now)
	return bucket.tokens >= 1
}

func (bucket *TokenBucket) Wait(now time.Time) time.Duration {
	bucket.refill(now)
	if bucket.tokens >= 1 || bucket.rate <= 0 {
		return 0
	}
	return time.Duration((1 - bucket.tokens) / bucket.rate * float64(time.Second))
}

func (bucket *TokenBucket) Reconfigure(rate float64, burst int) {
	bucket.rate = rate
	bucket.burst = float64(burst)
	bucket.tokens = min(bucket.tokens, bucket.burst)
}
