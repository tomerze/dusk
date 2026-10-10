package alerts

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"math/rand/v2"
	"net/http"
	"net/url"
	"os"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"
	"unicode"

	"github.com/jackc/pgx/v5"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/database"
)

const (
	TransitionOpened       = "opened"
	TransitionReEscalated  = "re_escalated"
	TransitionAcknowledged = "acknowledged"
	TransitionResolved     = "resolved"

	deliveryTick      = time.Second
	deliveryWorkers   = 16
	deliveryLease     = 2 * time.Minute
	expiryInterval    = time.Minute
	routingBatch      = 500
	retryBase         = 5 * time.Second
	retryCap          = 10 * time.Minute
	retryAfterCeiling = time.Hour
	requestTimeout    = 10 * time.Second
	recordTimeout     = 5 * time.Second
	errorExcerpt      = 300
)

var (
	deliveries = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_alert_deliveries_total",
		Help: "Alert notification attempts, by receiver, transition and outcome: delivered, retry or failed.",
	}, []string{"receiver", "transition", "outcome"})
	pendingDeliveries = promauto.NewGaugeVec(prometheus.GaugeOpts{
		Name: "twilight_alert_deliveries_pending",
		Help: "Alert notifications not yet delivered, by receiver, refreshed every 30 seconds.",
	}, []string{"receiver"})
)

type sender interface {
	send(operation context.Context, message notification) error
}

type deliveryError struct {
	message   string
	permanent bool
	after     time.Duration
}

func (failure deliveryError) Error() string {
	return failure.message
}

type notifier struct {
	settings config.Alerts
	instance string
	senders  map[string]sender
	names    []string
	horizon  time.Duration
}

func readSecret(path string) (string, error) {
	content, failure := os.ReadFile(path)
	if failure != nil {
		return "", fmt.Errorf("read the secret file %s: %w", path, failure)
	}
	secret := strings.TrimSpace(string(content))
	if secret == "" {
		return "", fmt.Errorf("the secret file %s is empty", path)
	}
	return secret, nil
}

func newNotifier(settings config.Alerts, instance string) (*notifier, error) {
	client := &http.Client{
		Timeout:       requestTimeout,
		CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
	}
	built := &notifier{settings: settings, instance: instance, senders: map[string]sender{}, horizon: time.Duration(settings.DeliveryHorizonSeconds) * time.Second}
	for _, receiver := range settings.Receivers {
		var secretFile string
		switch {
		case receiver.PagerDuty != nil:
			secretFile = receiver.PagerDuty.RoutingKeyFile
			built.senders[receiver.Name] = pagerDutySender{client: client, routingKeyFile: secretFile, url: receiver.PagerDuty.EventsURL()}
		case receiver.Slack != nil:
			secretFile = receiver.Slack.WebhookURLFile
			built.senders[receiver.Name] = slackSender{client: client, webhookURLFile: secretFile}
		case receiver.Webhook != nil:
			secretFile = receiver.Webhook.SecretFile
			built.senders[receiver.Name] = webhookSender{client: client, url: receiver.Webhook.URL, secretFile: secretFile}
		}
		if secretFile != "" {
			if _, failure := readSecret(secretFile); failure != nil {
				return nil, fmt.Errorf("alerts receiver %s: %w", receiver.Name, failure)
			}
		}
		built.names = append(built.names, receiver.Name)
	}
	return built, nil
}

func (built *notifier) match(severity Severity, kind string, tenant *string) []string {
	var matched []string
	for _, route := range built.settings.Routes {
		if route.Matches(string(severity), kind, tenant) && !slices.Contains(matched, route.Receiver) {
			matched = append(matched, route.Receiver)
		}
	}
	return matched
}

func backoff(attempts int) time.Duration {
	ceiling := retryCap
	if attempts < 20 {
		ceiling = min(retryCap, retryBase<<max(attempts-1, 0))
	}
	return max(time.Duration(rand.Int64N(int64(ceiling))), time.Second)
}

func printable(text string) string {
	cleaned := strings.Map(func(character rune) rune {
		if unicode.IsPrint(character) {
			return character
		}
		return ' '
	}, strings.TrimSpace(text))
	if runes := []rune(cleaned); len(runes) > errorExcerpt {
		return string(runes[:errorExcerpt]) + "..."
	}
	return cleaned
}

func postJSON(operation context.Context, client *http.Client, target string, body []byte, headers map[string]string) error {
	request, failure := http.NewRequestWithContext(operation, http.MethodPost, target, bytes.NewReader(body))
	if failure != nil {
		return deliveryError{message: "the receiver's URL is not valid", permanent: true}
	}
	request.Header.Set("Content-Type", "application/json")
	request.Header.Set("User-Agent", "twilight")
	for name, value := range headers {
		request.Header.Set(name, value)
	}
	response, failure := client.Do(request)
	if failure != nil {
		var wrapped *url.Error
		if errors.As(failure, &wrapped) {
			failure = wrapped.Err
		}
		return deliveryError{message: "the request failed: " + printable(failure.Error())}
	}
	defer response.Body.Close()
	if response.StatusCode >= 200 && response.StatusCode < 300 {
		return nil
	}
	excerpt, readFailure := io.ReadAll(io.LimitReader(response.Body, 4096))
	answered := deliveryError{message: fmt.Sprintf("HTTP %d %s", response.StatusCode, printable(string(excerpt)))}
	if readFailure != nil {
		answered.message += " (the rest of the answer could not be read: " + printable(readFailure.Error()) + ")"
	}
	switch {
	case response.StatusCode == http.StatusRequestTimeout, response.StatusCode == http.StatusTooEarly, response.StatusCode >= 500:
	case response.StatusCode == http.StatusTooManyRequests:
		if seconds, parsed := strconv.Atoi(strings.TrimSpace(response.Header.Get("Retry-After"))); parsed == nil && seconds > 0 {
			answered.after = min(time.Duration(seconds)*time.Second, retryAfterCeiling)
		}
	default:
		answered.permanent = true
	}
	return answered
}

func (store *Store) RunDelivery(operation context.Context) {
	var expired time.Time
	for {
		if failure := store.route(operation); failure != nil && operation.Err() == nil {
			store.logger.Warn("alert transitions were not routed to receivers", "error", failure)
		}
		if time.Since(expired) >= expiryInterval {
			expired = time.Now()
			if failure := store.expire(operation); failure != nil && operation.Err() == nil {
				store.logger.Warn("notifications past the delivery horizon were not closed", "error", failure)
			}
		}
		for len(store.notifier.names) > 0 && operation.Err() == nil {
			claimed, failure := store.claim(operation)
			if failure != nil {
				if operation.Err() == nil {
					store.logger.Warn("due alert notifications were not claimed", "error", failure)
				}
				break
			}
			var running sync.WaitGroup
			for _, claim := range claimed {
				running.Go(func() { store.deliver(operation, claim) })
			}
			running.Wait()
			if len(claimed) < deliveryWorkers {
				break
			}
		}
		select {
		case <-operation.Done():
			return
		case <-time.After(deliveryTick):
		}
	}
}

type pendingTransition struct {
	identifier int64
	alertID    int64
	transition string
	severity   Severity
	kind       string
	tenant     *string
	receivers  []string
}

func (store *Store) route(operation context.Context) error {
	var routed []pendingTransition
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		var locked bool
		if failure := transaction.QueryRow(operation, `select pg_try_advisory_xact_lock($1)`, database.AlertRoutingLockKey).Scan(&locked); failure != nil || !locked {
			return failure
		}
		rows, failure := transaction.Query(operation, `select transition.id, transition.alert_id, transition.transition, transition.severity, alert.kind, alert.tenant
			from alert_transitions transition join alerts alert on alert.id = transition.alert_id
			where transition.routed_at is null order by transition.id limit $1`, routingBatch)
		if failure != nil {
			return failure
		}
		pending, failure := pgx.CollectRows(rows, func(row pgx.CollectableRow) (pendingTransition, error) {
			var found pendingTransition
			return found, row.Scan(&found.identifier, &found.alertID, &found.transition, &found.severity, &found.kind, &found.tenant)
		})
		if failure != nil || len(pending) == 0 {
			return failure
		}
		identifiers := make([]int64, 0, len(pending))
		for index := range pending {
			entry := &pending[index]
			identifiers = append(identifiers, entry.identifier)
			if entry.transition == TransitionOpened || entry.transition == TransitionReEscalated {
				entry.receivers = store.notifier.match(entry.severity, entry.kind, entry.tenant)
			} else {
				rows, failure := transaction.Query(operation, `select distinct delivery.receiver from alert_deliveries delivery
					join alert_transitions transition on transition.id = delivery.transition_id
					where delivery.alert_id = $1 and transition.transition in ('opened', 're_escalated') order by delivery.receiver`, entry.alertID)
				if failure != nil {
					return failure
				}
				opened, failure := pgx.CollectRows(rows, pgx.RowTo[string])
				if failure != nil {
					return failure
				}
				for _, receiver := range opened {
					if _, configured := store.notifier.senders[receiver]; configured {
						entry.receivers = append(entry.receivers, receiver)
					}
				}
			}
			if len(entry.receivers) == 0 {
				continue
			}
			if _, failure := transaction.Exec(operation, `insert into alert_deliveries (transition_id, receiver, alert_id)
				select $1, receiver, $3 from unnest($2::text[]) as receiver on conflict do nothing`, entry.identifier, entry.receivers, entry.alertID); failure != nil {
				return failure
			}
		}
		if _, failure := transaction.Exec(operation, `update alert_transitions set routed_at = now() where id = any($1)`, identifiers); failure != nil {
			return failure
		}
		routed = pending
		return nil
	})
	if failure != nil {
		return failure
	}
	for _, entry := range routed {
		store.logger.Info("alert transition routed", "alert_id", entry.alertID, "transition", entry.transition, "severity", entry.severity,
			"kind", entry.kind, "tenant", entry.tenant, "receivers", entry.receivers)
	}
	return nil
}

type claim struct {
	transitionID int64
	receiver     string
	attempts     int
}

func (store *Store) claim(operation context.Context) ([]claim, error) {
	rows, failure := store.pool.Query(operation, `with due as (
			select delivery.transition_id, delivery.receiver from alert_deliveries delivery
			where delivery.state = 'pending' and delivery.next_attempt_at <= now() and delivery.receiver = any($1)
				and (delivery.leased_until is null or delivery.leased_until < now())
				and not exists (select 1 from alert_deliveries earlier where earlier.alert_id = delivery.alert_id and earlier.receiver = delivery.receiver
					and earlier.transition_id < delivery.transition_id and earlier.state = 'pending')
			order by delivery.next_attempt_at, delivery.transition_id
			limit $2
			for update of delivery skip locked
		)
		update alert_deliveries delivery set leased_by = $3, leased_until = now() + make_interval(secs => $4), attempts = delivery.attempts + 1,
			last_attempt_at = now()
		from due where delivery.transition_id = due.transition_id and delivery.receiver = due.receiver
		returning delivery.transition_id, delivery.receiver, delivery.attempts`,
		store.notifier.names, deliveryWorkers, store.notifier.instance, deliveryLease.Seconds())
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (claim, error) {
		var claimed claim
		return claimed, row.Scan(&claimed.transitionID, &claimed.receiver, &claimed.attempts)
	})
}

func (store *Store) load(operation context.Context, claimed claim) (notification, error) {
	message := notification{receiver: claimed.receiver, settings: store.notifier.settings}
	targets := append(alertTargets(&message.alert), &message.transition, &message.severity, &message.actor, &message.at, &message.key,
		&message.hostname, &message.campaignName)
	failure := store.pool.QueryRow(operation, `select `+prefixed("alert", alertColumns)+`, transition.transition, transition.severity, transition.actor, transition.at,
			delivery.idempotency_key::text, node.hostname, campaign.name
		from alert_deliveries delivery
		join alert_transitions transition on transition.id = delivery.transition_id
		join alerts alert on alert.id = delivery.alert_id
		left join nodes node on node.device_id = alert.detail ->> 'device_id' and node.installation_id = alert.detail ->> 'installation_id'
		left join campaigns campaign on campaign.id = case when alert.detail ->> 'campaign_id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
			then (alert.detail ->> 'campaign_id')::uuid end
		where delivery.transition_id = $1 and delivery.receiver = $2`, claimed.transitionID, claimed.receiver).Scan(targets...)
	if failure != nil {
		return message, failure
	}
	if failure := message.parseDetail(); failure != nil {
		store.logger.Warn("an alert's detail is not a JSON object; its notification carries no evidence", "alert_id", message.alert.ID, "receiver", claimed.receiver, "error", failure)
	}
	return message, nil
}

func prefixed(table, columns string) string {
	names := strings.Split(columns, ", ")
	for index, name := range names {
		names[index] = table + "." + name
	}
	return strings.Join(names, ", ")
}

func (store *Store) deliver(operation context.Context, claimed claim) {
	message, failure := store.load(operation, claimed)
	if failure == nil {
		attempt, cancel := context.WithTimeout(operation, deliveryLease/2)
		failure = store.notifier.senders[claimed.receiver].send(attempt, message)
		cancel()
	}
	recording, cancel := context.WithTimeout(context.WithoutCancel(operation), recordTimeout)
	defer cancel()
	fields := []any{"alert_id", message.alert.ID, "transition", message.transition, "receiver", claimed.receiver, "attempts", claimed.attempts}
	if operation.Err() != nil {
		if _, released := store.pool.Exec(recording, `update alert_deliveries set leased_by = null, leased_until = null, next_attempt_at = now()
			where transition_id = $1 and receiver = $2 and leased_by = $3 and attempts = $4`, claimed.transitionID, claimed.receiver, store.notifier.instance, claimed.attempts); released != nil {
			store.logger.Warn("an alert notification interrupted by shutdown was not released; it is retried when its lease ends", append(fields, "error", released)...)
		}
		return
	}
	var deliveryFailure deliveryError
	var state string
	var record error
	lease := []any{claimed.transitionID, claimed.receiver, store.notifier.instance, claimed.attempts}
	switch {
	case failure == nil:
		record = store.pool.QueryRow(recording, `update alert_deliveries set state = 'delivered', delivered_at = now(), last_error = null, leased_by = null, leased_until = null
			where transition_id = $1 and receiver = $2 and leased_by = $3 and attempts = $4 returning state`, lease...).Scan(&state)
	case errors.As(failure, &deliveryFailure) && deliveryFailure.permanent:
		fields = append(fields, "error", deliveryFailure.message)
		record = store.pool.QueryRow(recording, `update alert_deliveries set state = 'failed', last_error = $5, leased_by = null, leased_until = null
			where transition_id = $1 and receiver = $2 and leased_by = $3 and attempts = $4 returning state`, append(lease, deliveryFailure.message)...).Scan(&state)
	default:
		delay := max(backoff(claimed.attempts), deliveryFailure.after)
		reason := printable(failure.Error())
		fields = append(fields, "error", reason, "retry_in_seconds", int(delay.Seconds()))
		record = store.pool.QueryRow(recording, `update alert_deliveries set last_error = $5, leased_by = null, leased_until = null,
				next_attempt_at = now() + make_interval(secs => $6),
				state = case when created_at + make_interval(secs => $7) <= now() + make_interval(secs => $6) then 'failed' else 'pending' end
			where transition_id = $1 and receiver = $2 and leased_by = $3 and attempts = $4 returning state`,
			append(lease, reason, delay.Seconds(), store.notifier.horizon.Seconds())...).Scan(&state)
	}
	switch {
	case errors.Is(record, pgx.ErrNoRows):
		store.logger.Warn("an alert notification's lease ended before its outcome was recorded; another instance retries it", fields...)
	case record != nil:
		store.logger.Warn("an alert notification's outcome was not recorded; it is retried when its lease ends", append(fields, "record_error", record)...)
	case state == "delivered":
		deliveries.WithLabelValues(claimed.receiver, message.transition, "delivered").Inc()
		store.logger.Info("alert notification delivered", fields...)
	case state == "failed":
		deliveries.WithLabelValues(claimed.receiver, message.transition, "failed").Inc()
		store.logger.Error("alert notification gave up", fields...)
	default:
		deliveries.WithLabelValues(claimed.receiver, message.transition, "retry").Inc()
		store.logger.Warn("alert notification failed; retrying", fields...)
	}
}

func (store *Store) expire(operation context.Context) error {
	rows, failure := store.pool.Query(operation, `update alert_deliveries delivery set state = 'failed',
			last_error = concat_ws('; ', delivery.last_error, 'not delivered within alerts.delivery_horizon_seconds')
		from alert_transitions transition
		where transition.id = delivery.transition_id and delivery.state = 'pending' and delivery.created_at <= now() - make_interval(secs => $1)
			and (delivery.leased_until is null or delivery.leased_until < now())
		returning delivery.alert_id, transition.transition, delivery.receiver, delivery.attempts`, store.notifier.horizon.Seconds())
	if failure != nil {
		return failure
	}
	var alertID int64
	var transition, receiver string
	var attempts int
	_, failure = pgx.ForEachRow(rows, []any{&alertID, &transition, &receiver, &attempts}, func() error {
		deliveries.WithLabelValues(receiver, transition, "failed").Inc()
		store.logger.Error("alert notification gave up", "alert_id", alertID, "transition", transition, "receiver", receiver, "attempts", attempts,
			"error", "not delivered within alerts.delivery_horizon_seconds")
		return nil
	})
	return failure
}
