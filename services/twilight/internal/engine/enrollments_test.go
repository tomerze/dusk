package engine

import (
	"testing"
	"time"
)

func TestEnrollmentRateCountsEveryMinuteOnItsOwn(test *testing.T) {
	rate := newEnrollmentRate(2)
	base := time.Date(2026, 10, 9, 12, 0, 30, 0, time.UTC)
	crossings := 0
	for _, offset := range []time.Duration{0, time.Minute, 0, time.Minute, 0, time.Minute, time.Minute} {
		if _, crossed := rate.count(base.Add(offset)); crossed {
			crossings++
		}
	}
	if crossings != 2 {
		test.Fatalf("two minutes interleaved across partitions each passed the threshold; %d alerts", crossings)
	}
	rate.count(base.Add(time.Hour))
	if _, crossed := rate.count(base); crossed || len(rate.minutes) != 1 {
		test.Fatalf("a minute past the kept window was counted, or old minutes were kept: %v", rate.minutes)
	}
}
