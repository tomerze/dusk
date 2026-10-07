package kafka

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"strings"
	"time"

	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"
	"github.com/twmb/franz-go/pkg/sasl/scram"

	"dusk/services/twilight/internal/config"
)

func Options(settings config.Kafka, instance string) ([]kgo.Opt, error) {
	options := []kgo.Opt{
		kgo.SeedBrokers(settings.Brokers...),
		kgo.ClientID(settings.ClientID + "-" + instance),
		kgo.DialTimeout(10 * time.Second),
		kgo.RequestTimeoutOverhead(10 * time.Second),
		kgo.RetryTimeout(30 * time.Second),
		kgo.MetadataMaxAge(time.Minute),
	}
	if settings.TLS.CA != "" {
		pool := x509.NewCertPool()
		authorities, failure := os.ReadFile(settings.TLS.CA)
		if failure != nil {
			return nil, fmt.Errorf("read kafka.tls.ca: %w", failure)
		}
		if !pool.AppendCertsFromPEM(authorities) {
			return nil, fmt.Errorf("kafka.tls.ca %s holds no certificate", settings.TLS.CA)
		}
		tlsConfig := &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12, ServerName: settings.TLS.ServerName}
		if settings.TLS.Certificate != "" {
			certificatePath, keyPath := settings.TLS.Certificate, settings.TLS.Key
			tlsConfig.GetClientCertificate = func(*tls.CertificateRequestInfo) (*tls.Certificate, error) {
				pair, failure := tls.LoadX509KeyPair(certificatePath, keyPath)
				if failure != nil {
					return nil, fmt.Errorf("load the Kafka client certificate: %w", failure)
				}
				return &pair, nil
			}
		}
		options = append(options, kgo.Dialer((&tls.Dialer{NetDialer: &net.Dialer{Timeout: 10 * time.Second, KeepAlive: 30 * time.Second}, Config: tlsConfig}).DialContext))
	} else if !settings.AllowPlaintext {
		return nil, errors.New("kafka.tls.ca is required unless kafka.allow_plaintext is true")
	}
	if settings.SASL.Mechanism != "" {
		password, failure := os.ReadFile(settings.SASL.PasswordFile)
		if failure != nil {
			return nil, fmt.Errorf("read kafka.sasl.password_file: %w", failure)
		}
		authentication := scram.Auth{User: settings.SASL.Username, Pass: strings.TrimRight(string(password), "\r\n")}
		switch settings.SASL.Mechanism {
		case "SCRAM-SHA-256":
			options = append(options, kgo.SASL(authentication.AsSha256Mechanism()))
		case "SCRAM-SHA-512":
			options = append(options, kgo.SASL(authentication.AsSha512Mechanism()))
		default:
			return nil, fmt.Errorf("unsupported kafka.sasl.mechanism %q", settings.SASL.Mechanism)
		}
	}
	return options, nil
}

type TopicRequirement struct {
	Topic         string
	CleanupPolicy string
}

func RequiredTopics(topics config.Topics) []TopicRequirement {
	return []TopicRequirement{
		{topics.Connections, "delete"},
		{topics.Census, "compact"},
		{topics.Ledger, "delete"},
		{topics.Enrollments, "delete"},
		{topics.NodeState, "compact"},
		{topics.ProcessResults, "delete"},
	}
}

func CheckTopics(operation context.Context, client *kgo.Client, requirements []TopicRequirement) error {
	admin := kadm.NewClient(client)
	names := make([]string, len(requirements))
	for index, requirement := range requirements {
		names[index] = requirement.Topic
	}
	listed, failure := admin.ListTopics(operation, names...)
	if failure != nil {
		return fmt.Errorf("list topics: %w", failure)
	}
	var problems []string
	var present []string
	for _, name := range names {
		if detail, found := listed[name]; found && detail.Err == nil {
			present = append(present, name)
		}
	}
	described := kadm.ResourceConfigs{}
	if len(present) > 0 {
		described, failure = admin.DescribeTopicConfigs(operation, present...)
		if failure != nil {
			return fmt.Errorf("describe topics: %w", failure)
		}
	}
	for _, requirement := range requirements {
		found, failure := described.On(requirement.Topic, nil)
		if failure != nil {
			problems = append(problems, fmt.Sprintf("%s is missing", requirement.Topic))
			continue
		}
		if found.Err != nil {
			problems = append(problems, fmt.Sprintf("%s: %v", requirement.Topic, found.Err))
			continue
		}
		policy := ""
		for _, entry := range found.Configs {
			if entry.Key == "cleanup.policy" {
				policy = entry.MaybeValue()
			}
		}
		if policy != requirement.CleanupPolicy {
			problems = append(problems, fmt.Sprintf("%s has cleanup.policy %q, want %q", requirement.Topic, policy, requirement.CleanupPolicy))
		}
	}
	if len(problems) > 0 {
		return errors.New(strings.Join(problems, "; "))
	}
	return nil
}

type Producer struct {
	client    *kgo.Client
	validator *Validator
	topic     string
}

func NewNodeStateProducer(client *kgo.Client, validator *Validator, topic string) *Producer {
	return &Producer{client: client, validator: validator, topic: topic}
}

func (producer *Producer) ProduceNodeState(operation context.Context, state NodeState) error {
	value, failure := json.Marshal(state)
	if failure != nil {
		return failure
	}
	if failure := producer.validator.Validate(ContractNodeState, value); failure != nil {
		return fmt.Errorf("node-state message fails its contract: %w", failure)
	}
	record := &kgo.Record{Topic: producer.topic, Key: []byte(state.Key()), Value: value}
	return producer.client.ProduceSync(operation, record).FirstErr()
}

func (producer *Producer) ClearNodeState(operation context.Context, key string) error {
	return producer.client.ProduceSync(operation, &kgo.Record{Topic: producer.topic, Key: []byte(key)}).FirstErr()
}
