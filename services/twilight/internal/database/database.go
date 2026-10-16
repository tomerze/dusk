package database

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/jackc/pgx/v5/pgxpool"
)

const (
	migrationLockKey    int64 = 0x7477696c69676d01
	partitionLockKey    int64 = 0x7477696c69677002
	LeaderLockKey       int64 = 0x7477696c69676c03
	AlertRoutingLockKey int64 = 0x7477696c69676104
)

func Open(operation context.Context, url string, maximumConnections int32) (*pgxpool.Pool, error) {
	config, failure := pgxpool.ParseConfig(url)
	if failure != nil {
		return nil, fmt.Errorf("parse database url: %w", failure)
	}
	if maximumConnections > 0 {
		config.MaxConns = maximumConnections
	}
	if config.ConnConfig.ConnectTimeout == 0 {
		config.ConnConfig.ConnectTimeout = 10 * time.Second
	}
	if _, set := config.ConnConfig.RuntimeParams["application_name"]; !set {
		config.ConnConfig.RuntimeParams["application_name"] = "twilight"
	}
	if _, set := config.ConnConfig.RuntimeParams["statement_timeout"]; !set {
		config.ConnConfig.RuntimeParams["statement_timeout"] = "60000"
	}
	config.HealthCheckPeriod = 15 * time.Second
	config.MaxConnIdleTime = 5 * time.Minute
	pool, failure := pgxpool.NewWithConfig(operation, config)
	if failure != nil {
		return nil, fmt.Errorf("open database pool: %w", failure)
	}
	pinging, cancel := context.WithTimeout(operation, 15*time.Second)
	defer cancel()
	if failure := pool.Ping(pinging); failure != nil {
		pool.Close()
		return nil, fmt.Errorf("reach database: %w", failure)
	}
	return pool, nil
}

var (
	escapedNul       = []byte(`\u0000`)
	escapedSurrogate = []byte(`\ud`)
	upperSurrogate   = []byte(`\uD`)
)

func Storable(value []byte) []byte {
	if utf8.Valid(value) && !bytes.Contains(value, escapedNul) && !bytes.Contains(value, escapedSurrogate) && !bytes.Contains(value, upperSurrogate) {
		return value
	}
	decoder := json.NewDecoder(bytes.NewReader(value))
	decoder.UseNumber()
	var document any
	if decoder.Decode(&document) != nil {
		return value
	}
	var encoded bytes.Buffer
	encoder := json.NewEncoder(&encoded)
	encoder.SetEscapeHTML(false)
	if encoder.Encode(withoutNul(document)) != nil {
		return value
	}
	return encoded.Bytes()
}

func withoutNul(value any) any {
	switch typed := value.(type) {
	case string:
		return strings.ReplaceAll(typed, "\x00", "\uFFFD")
	case []any:
		for index, item := range typed {
			typed[index] = withoutNul(item)
		}
	case map[string]any:
		cleaned := make(map[string]any, len(typed))
		for key, item := range typed {
			cleaned[strings.ReplaceAll(key, "\x00", "\uFFFD")] = withoutNul(item)
		}
		return cleaned
	}
	return value
}
