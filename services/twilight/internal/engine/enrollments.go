package engine

import (
	"context"
	"fmt"
	"time"

	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
)

func (engine *Engine) runEnrollments(operation context.Context, term int64) {
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
				Tenant: enrollment.Tenant, DuskVersion: enrollment.DuskVersion, Impl: enrollment.Impl, TargetArch: enrollment.TargetArch, Hostname: enrollment.Hostname, At: at}); failure != nil {
				return fmt.Errorf("record the enrollment of %s in inventory: %w", key, failure)
			}
			engine.Logger.Info("enrollment recorded", "operation", enrollment.Operation, "device_id", key.DeviceID, "installation_id", key.InstallationID)
		}
		return nil
	})
}
