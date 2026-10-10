package api

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"time"
)

const (
	keepaliveInterval  = 15 * time.Second
	streamWriteTimeout = 10 * time.Second
)

func (server *Server) stream(call *exchange) error {
	if server.streams.Add(1) > int64(server.MaxStreams) {
		server.streams.Add(-1)
		server.Logger.Warn("a server-sent event stream was refused at the limit", "max_streams", server.MaxStreams, "principal", call.actor())
		return newProblem(http.StatusServiceUnavailable, "too_many_streams", "this instance serves as many event streams as it may; try again shortly").with("retry_after_seconds", 30)
	}
	streamsGauge.Inc()
	defer func() {
		server.streams.Add(-1)
		streamsGauge.Dec()
	}()
	operation, cancel := context.WithTimeout(call.request.Context(), server.StreamLifetime)
	defer cancel()
	events := server.Backend.Subscribe(operation)
	controller := http.NewResponseController(call.writer)
	header := call.writer.Header()
	header.Set("Content-Type", "text/event-stream")
	header.Set("X-Accel-Buffering", "no")
	call.writer.WriteHeader(http.StatusOK)
	write := func(text string) bool {
		if failure := controller.SetWriteDeadline(time.Now().Add(streamWriteTimeout)); failure != nil && !errors.Is(failure, http.ErrNotSupported) {
			return false
		}
		if _, failure := fmt.Fprint(call.writer, text); failure != nil {
			return false
		}
		if controller.Flush() != nil {
			return false
		}
		if failure := controller.SetWriteDeadline(time.Time{}); failure != nil && !errors.Is(failure, http.ErrNotSupported) {
			return false
		}
		return true
	}
	if !write("retry: 5000\n\n") {
		return nil
	}
	server.Logger.Debug("event stream opened", "principal", call.actor(), "request_id", call.requestID)
	keepalive := time.NewTicker(keepaliveInterval)
	defer keepalive.Stop()
	for {
		select {
		case event, open := <-events:
			if !open {
				return nil
			}
			encoded, failure := json.Marshal(event)
			if failure != nil {
				server.Logger.Warn("a feed event could not be encoded for a stream", "kind", event.Kind, "error", failure)
				continue
			}
			if !write("event: " + event.Kind + "\ndata: " + string(encoded) + "\n\n") {
				return nil
			}
		case <-keepalive.C:
			if !write(": keepalive\n\n") {
				return nil
			}
		case <-server.draining:
			return nil
		case <-operation.Done():
			return nil
		}
	}
}
