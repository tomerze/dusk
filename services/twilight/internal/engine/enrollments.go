package engine

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
)

type enrollmentRate struct {
	mutex     sync.Mutex
	threshold int
	newest    time.Time
	minutes   map[time.Time]int
}

func newEnrollmentRate(threshold int) *enrollmentRate {
	return &enrollmentRate{threshold: threshold, minutes: map[time.Time]int{}}
}

func (rate *enrollmentRate) count(at time.Time) (time.Time, bool) {
	minute := at.UTC().Truncate(time.Minute)
	rate.mutex.Lock()
	defer rate.mutex.Unlock()
	horizon := enrollmentMinutesKept * time.Minute
	if minute.After(rate.newest) {
		rate.newest = minute
		for kept := range rate.minutes {
			if kept.Before(minute.Add(-horizon)) {
				delete(rate.minutes, kept)
			}
		}
	}
	if minute.Before(rate.newest.Add(-horizon)) {
		return minute, false
	}
	rate.minutes[minute]++
	return minute, rate.minutes[minute] == rate.threshold+1
}

const enrollmentMinutesKept = 10

func (engine *Engine) runEnrollments(operation context.Context, term int64) {
	threshold := engine.Config.Alerts.EnrollmentRatePerMinute
	rate := newEnrollmentRate(threshold)
	kafka.ConsumeGroup(operation, engine.KafkaOptions, "enrollments", engine.Config.Kafka.InventoryGroup, engine.Config.Kafka.Topics.Enrollments, engine.Logger.With("term", term), func(operation context.Context, record *kgo.Record) error {
		if record.Value == nil {
			return nil
		}
		enrollment, valid := kafka.Decode[kafka.Enrollment](engine.Validator, kafka.ContractEnrollments, record, engine.Logger)
		if !valid {
			return nil
		}
		at, failure := kafka.ParseTime(enrollment.Time)
		if failure != nil {
			at = time.Now()
		}
		if enrollment.Outcome == "issued" && enrollment.DeviceID != nil && enrollment.InstallationID != nil && (enrollment.Operation == "enroll" || enrollment.Operation == "renew") {
			key := NodeKey{DeviceID: *enrollment.DeviceID, InstallationID: *enrollment.InstallationID}
			if failure := engine.Inventory.RecordEnrollment(operation, inventory.Enrollment{Key: key, Operation: enrollment.Operation, CertFingerprint: enrollment.CertFingerprint,
				Tenant: enrollment.Tenant, DuskVersion: enrollment.DuskVersion, Impl: enrollment.Impl, TargetArch: enrollment.TargetArch, Hostname: enrollment.Hostname,
				CredentialKind: enrollment.CredentialKind, CredentialRef: enrollment.CredentialRef, CredentialIssuer: enrollment.CredentialIssuer, At: at}); failure != nil {
				return fmt.Errorf("record the enrollment of %s in inventory: %w", key, failure)
			}
			engine.Logger.Info("enrollment recorded", "operation", enrollment.Operation, "device_id", key.DeviceID, "installation_id", key.InstallationID)
		}
		if minute, crossed := rate.count(at); crossed {
			if _, failure := engine.Alerts.Raise(operation, alerts.Raised{Severity: alerts.High, Kind: alerts.KindEnrollmentRate, Fingerprint: alerts.KindEnrollmentRate,
				Detail: map[string]any{"minute": minute.Format(time.RFC3339), "threshold": threshold, "message": "more enrollments in one minute than alerts.enrollment_rate_per_minute"}, At: at}); failure != nil {
				engine.Logger.Warn("the enrollment rate alert was not raised", "minute", minute.Format(time.RFC3339), "error", failure)
			}
		}
		return nil
	})
}
