package kafka

import (
	"bytes"
	"embed"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"strings"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/santhosh-tekuri/jsonschema/v6"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/database"
)

//go:embed contracts/*.schema.json
var contracts embed.FS

const (
	ContractConnections    = "dusk.connections"
	ContractCensus         = "dusk.census"
	ContractLedger         = "dusk.ledger"
	ContractEnrollments    = "dusk.enrollments"
	ContractNodeState      = "dusk.node-state"
	ContractProcessResults = "dusk.process-results"
	ContractProcessOutput  = "dusk.process-output"
	ContractFiles          = "dusk.files"
	ContractIntended       = "dusk.intended-processes"
)

var invalidMessages = promauto.NewCounterVec(prometheus.CounterOpts{
	Name: "twilight_kafka_invalid_messages_total",
	Help: "Kafka records dropped because they failed their contract schema.",
}, []string{"topic"})

func CountInvalid(topic string) {
	invalidMessages.WithLabelValues(topic).Inc()
}

type Validator struct {
	schemas map[string]*jsonschema.Schema
}

type refusingLoader struct{}

func (refusingLoader) Load(url string) (any, error) {
	return nil, fmt.Errorf("schema %s is not one of the embedded contracts", url)
}

func NewValidator() (*Validator, error) {
	compiler := jsonschema.NewCompiler()
	compiler.DefaultDraft(jsonschema.Draft2020)
	compiler.AssertFormat()
	compiler.UseLoader(refusingLoader{})
	entries, failure := fs.ReadDir(contracts, "contracts")
	if failure != nil {
		return nil, failure
	}
	var topics []string
	for _, entry := range entries {
		content, failure := contracts.ReadFile("contracts/" + entry.Name())
		if failure != nil {
			return nil, failure
		}
		document, failure := jsonschema.UnmarshalJSON(bytes.NewReader(content))
		if failure != nil {
			return nil, fmt.Errorf("contract %s: %w", entry.Name(), failure)
		}
		if failure := compiler.AddResource("file:///contracts/"+entry.Name(), document); failure != nil {
			return nil, fmt.Errorf("contract %s: %w", entry.Name(), failure)
		}
		if topic, found := strings.CutSuffix(entry.Name(), ".schema.json"); found && topic != "common" {
			topics = append(topics, topic)
		}
	}
	validator := &Validator{schemas: map[string]*jsonschema.Schema{}}
	for _, topic := range topics {
		schema, failure := compiler.Compile("file:///contracts/" + topic + ".schema.json")
		if failure != nil {
			return nil, fmt.Errorf("compile contract %s: %w", topic, failure)
		}
		validator.schemas[topic] = schema
	}
	return validator, nil
}

var ErrUnknownContract = errors.New("no contract for this topic")

func (validator *Validator) Validate(contract string, value []byte) error {
	schema, known := validator.schemas[contract]
	if !known {
		return fmt.Errorf("%w: %s", ErrUnknownContract, contract)
	}
	document, failure := jsonschema.UnmarshalJSON(bytes.NewReader(value))
	if failure != nil {
		return fmt.Errorf("not JSON: %w", failure)
	}
	return schema.Validate(document)
}

func Decode[Message any](validator *Validator, contract string, record *kgo.Record, logger *slog.Logger) (Message, bool) {
	var message Message
	if failure := validator.Validate(contract, record.Value); failure != nil {
		invalidMessages.WithLabelValues(record.Topic).Inc()
		logger.Warn("dropped a Kafka record that fails its contract",
			"topic", record.Topic, "partition", record.Partition, "offset", record.Offset, "contract", contract, "error", failure)
		return message, false
	}
	if failure := json.Unmarshal(database.Storable(record.Value), &message); failure != nil {
		invalidMessages.WithLabelValues(record.Topic).Inc()
		logger.Warn("dropped a Kafka record that does not decode",
			"topic", record.Topic, "partition", record.Partition, "offset", record.Offset, "contract", contract, "error", failure)
		return message, false
	}
	return message, true
}
