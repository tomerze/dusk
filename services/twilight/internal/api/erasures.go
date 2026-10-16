package api

import (
	"errors"
	"net/http"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/erasure"
)

const exportPath = "/api/v1/devices/{device}/export"

func (server *Server) eraseDevice(call *exchange) error {
	device := call.request.PathValue("device")
	if !identifierPattern.MatchString(device) {
		return invalid("a device id is 32 lowercase hexadecimal digits")
	}
	var body struct {
		Reason string `json:"reason"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	requested, created, failure := server.Backend.EraseDevice(call.request.Context(), device, body.Reason, call.actor())
	return answerErasure(call, requested, created, failure)
}

func (server *Server) eraseTenant(call *exchange) error {
	tenant := call.request.PathValue("tenant")
	if !config.TenantPattern.MatchString(tenant) {
		return invalid("a tenant is 1 to 63 of a-z, 0-9 and -")
	}
	var body struct {
		Reason string `json:"reason"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	requested, created, failure := server.Backend.EraseTenant(call.request.Context(), tenant, body.Reason, call.actor())
	return answerErasure(call, requested, created, failure)
}

func answerErasure(call *exchange, requested erasure.Erasure, created bool, failure error) error {
	if failure != nil {
		return failure
	}
	if created {
		call.writer.Header().Set("Location", "/api/v1/erasures/"+requested.ID.String())
	}
	writeJSON(call, http.StatusAccepted, requested)
	return nil
}

func (server *Server) erasure(call *exchange) error {
	identifier, failure := uuid.Parse(call.request.PathValue("id"))
	if failure != nil {
		return invalid("an erasure id is a UUID")
	}
	found, failure := server.Backend.Erasure(call.request.Context(), identifier)
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, found)
	return nil
}

func (server *Server) exportDevice(call *exchange) error {
	device := call.request.PathValue("device")
	if !identifierPattern.MatchString(device) {
		return invalid("a device id is 32 lowercase hexadecimal digits")
	}
	var body struct {
		Reason string `json:"reason"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	if _, failure := erasure.ValidReason(body.Reason); failure != nil {
		return failure
	}
	select {
	case server.exports <- struct{}{}:
		defer func() { <-server.exports }()
	default:
		return newProblem(http.StatusTooManyRequests, "export_running", "this instance is already exporting device data; try again when that export ends").with("retry_after_seconds", 60)
	}
	controller := http.NewResponseController(call.writer)
	if failure := controller.SetWriteDeadline(time.Time{}); failure != nil && !errors.Is(failure, http.ErrNotSupported) {
		return failure
	}
	header := call.writer.Header()
	header.Set("Content-Type", "application/x-ndjson")
	header.Set("Content-Disposition", `attachment; filename="device-`+device+`.ndjson"`)
	header.Set("X-Accel-Buffering", "no")
	call.writer.WriteHeader(http.StatusOK)
	flush := func() {
		if failure := controller.Flush(); failure != nil && !errors.Is(failure, http.ErrNotSupported) {
			server.Logger.Debug("an export flush failed", "request_id", call.requestID, "error", failure)
		}
	}
	if failure := server.Backend.ExportDevice(call.request.Context(), device, body.Reason, call.actor(), call.writer, flush); failure != nil {
		server.Logger.Error("a device data export ended early; the client received an incomplete export without its summary", "device_id", device,
			"principal", call.actor(), "request_id", call.requestID, "error", failure)
	}
	return nil
}
