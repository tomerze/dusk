package alerts

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
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
	queueWorkers      = 4
	deliveryLease     = 2 * time.Minute
	expiryInterval    = time.Minute
	routingBatch      = 500
	digestWindow      = time.Minute
	digestExamples    = 5
	retryBase         = 5 * time.Second
	retryCap          = 10 * time.Minute
	retryAfterCeiling = time.Hour
	requestTimeout    = 10 * time.Second
	recordTimeout     = 5 * time.Second
	errorExcerpt      = 300
	removedReceiver   = "the receiver is no longer in alerts.receivers"
)

var (
	deliveries = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_alert_deliveries_total",
		Help: "Alert notification attempts, by receiver, transition and outcome: delivered, retry or failed; a digest counts once for each alert it carries.",
	}, []string{"receiver", "transition", "outcome"})
	pendingDeliveries = promauto.NewGaugeVec(prometheus.GaugeOpts{
		Name: "twilight_alert_deliveries_pending",
		Help: "Alert notifications not yet delivered, by receiver, refreshed every 30 seconds.",
	}, []string{"receiver"})
)

type sender interface {
	send(operation context.Context, message notification) error
	ratePerMinute() int
	digests() bool
}

type deliveryError struct {
	message   string
	permanent bool
	limited   bool
	after     time.Duration
}

func (failure deliveryError) Error() string {
	return failure.message
}

type queue struct {
	name     string
	sender   sender
	digests  bool
	interval time.Duration
	burst    int
}

type notifier struct {
	settings config.Alerts
	instance string
	senders  map[string]sender
	queues   map[string]*queue
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

func newNotifier(settings config.Alerts, instance string, logger *slog.Logger) (*notifier, error) {
	client := &http.Client{
		Timeout:       requestTimeout,
		CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
	}
	built := &notifier{settings: settings, instance: instance, senders: map[string]sender{}, queues: map[string]*queue{},
		horizon: time.Duration(settings.DeliveryHorizonSeconds) * time.Second}
	for _, receiver := range settings.Receivers {
		var secretFile string
		switch {
		case receiver.PagerDuty != nil:
			secretFile = receiver.PagerDuty.RoutingKeyFile
			built.senders[receiver.Name] = pagerDutySender{client: client, routingKeyFile: secretFile, url: receiver.PagerDuty.EventsURL()}
		case receiver.Slack != nil:
			secretFile = receiver.Slack.WebhookURLFile
			built.senders[receiver.Name] = slackSender{client: client, webhookURLFile: secretFile}
		case receiver.Teams != nil:
			secretFile = receiver.Teams.WebhookURLFile
			built.senders[receiver.Name] = teamsSender{client: client, webhookURLFile: secretFile}
		case receiver.Webhook != nil:
			secretFile = receiver.Webhook.SecretFile
			built.senders[receiver.Name] = webhookSender{client: client, url: receiver.Webhook.URL, secretFile: secretFile}
		case receiver.Email != nil:
			secretFile = receiver.Email.PasswordFile
			built.senders[receiver.Name] = emailSender{settings: *receiver.Email, logger: logger}
		}
		if secretFile != "" {
			if _, failure := readSecret(secretFile); failure != nil {
				return nil, fmt.Errorf("alerts receiver %s: %w", receiver.Name, failure)
			}
		}
		sending := built.senders[receiver.Name]
		rate := sending.ratePerMinute()
		if receiver.RatePerMinute > 0 {
			rate = receiver.RatePerMinute
		}
		built.queues[receiver.Name] = &queue{name: receiver.Name, sender: sending, digests: sending.digests(),
			interval: time.Minute / time.Duration(rate), burst: (rate + 59) / 60}
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
		answered.limited = true
		if seconds, parsed := strconv.Atoi(strings.TrimSpace(response.Header.Get("Retry-After"))); parsed == nil && seconds > 0 {
			answered.after = min(time.Duration(seconds)*time.Second, retryAfterCeiling)
		}
	default:
		answered.permanent = true
	}
	return answered
}

func (store *Store) RunDelivery(operation context.Context) {
	store.routeAll(operation)
	var queues sync.WaitGroup
	defer queues.Wait()
	for _, name := range store.notifier.names {
		target := store.notifier.queues[name]
		queues.Go(func() { store.deliverFrom(operation, target) })
	}
	var expired time.Time
	closedRemoved := false
	for {
		if !closedRemoved {
			failure := store.failRemoved(operation)
			closedRemoved = failure == nil
			if failure != nil && operation.Err() == nil {
				store.logger.Warn("notifications for receivers no longer in alerts.receivers were not closed", "error", failure)
			}
		}
		store.routeAll(operation)
		if time.Since(expired) >= expiryInterval {
			expired = time.Now()
			if failure := store.expire(operation); failure != nil && operation.Err() == nil {
				store.logger.Warn("notifications past the delivery horizon were not closed", "error", failure)
			}
		}
		select {
		case <-operation.Done():
			return
		case <-time.After(deliveryTick):
		}
	}
}

func (store *Store) routeAll(operation context.Context) {
	for operation.Err() == nil {
		routed, failure := store.route(operation)
		if failure != nil {
			if operation.Err() == nil {
				store.logger.Warn("alert transitions were not routed to receivers", "error", failure)
			}
			return
		}
		if routed < routingBatch {
			return
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

func (store *Store) route(operation context.Context) (int, error) {
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
		var transitions, alertIdentifiers []int64
		var receivers, kinds, changes, severities []string
		var digests []bool
		for index := range pending {
			entry := &pending[index]
			identifiers = append(identifiers, entry.identifier)
			if entry.transition == TransitionOpened || entry.transition == TransitionReEscalated {
				entry.receivers = store.notifier.match(entry.severity, entry.kind, entry.tenant)
			} else {
				rows, failure := transaction.Query(operation, `select distinct delivery.receiver from alert_deliveries delivery
					where delivery.alert_id = $1 and delivery.transition in ('opened', 're_escalated') order by delivery.receiver`, entry.alertID)
				if failure != nil {
					return failure
				}
				opened, failure := pgx.CollectRows(rows, pgx.RowTo[string])
				if failure != nil {
					return failure
				}
				for _, receiver := range opened {
					if _, configured := store.notifier.queues[receiver]; configured {
						entry.receivers = append(entry.receivers, receiver)
					}
				}
			}
			for _, receiver := range entry.receivers {
				transitions = append(transitions, entry.identifier)
				alertIdentifiers = append(alertIdentifiers, entry.alertID)
				receivers = append(receivers, receiver)
				kinds = append(kinds, entry.kind)
				changes = append(changes, entry.transition)
				severities = append(severities, string(entry.severity))
				digests = append(digests, store.notifier.queues[receiver].digests)
			}
		}
		if len(receivers) > 0 {
			if _, failure := transaction.Exec(operation, `insert into alert_deliveries (transition_id, receiver, alert_id, kind, transition, severity, next_attempt_at)
				select routed.transition_id, routed.receiver, routed.alert_id, routed.kind, routed.transition, routed.severity,
					greatest(now(), case when routed.digests then (select max(earlier.last_attempt_at) from alert_deliveries earlier
						where earlier.receiver = routed.receiver and earlier.kind = routed.kind and earlier.transition = routed.transition) + make_interval(secs => $8) end)
				from unnest($1::bigint[], $2::text[], $3::bigint[], $4::text[], $5::text[], $6::text[], $7::boolean[])
					as routed (transition_id, receiver, alert_id, kind, transition, severity, digests)
				on conflict do nothing`, transitions, receivers, alertIdentifiers, kinds, changes, severities, digests, digestWindow.Seconds()); failure != nil {
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
		return 0, failure
	}
	for _, entry := range routed {
		store.logger.Info("alert transition routed", "alert_id", entry.alertID, "transition", entry.transition, "severity", entry.severity,
			"kind", entry.kind, "tenant", entry.tenant, "receivers", entry.receivers)
	}
	return len(routed), nil
}

type group struct {
	lease        string
	count        int
	attempts     int
	transitionID int64
	kind         string
	transition   string
	severity     Severity
}

const dueDeliveries = `select transition_id, kind, transition, alert_severity_rank(severity) as rank, next_attempt_at from (
		select delivery.transition_id, delivery.kind, delivery.transition, delivery.severity, delivery.next_attempt_at, delivery.leased_until,
			delivery.transition_id = min(delivery.transition_id) over (partition by delivery.alert_id) as earliest
		from alert_deliveries delivery where delivery.receiver = $1 and delivery.state = 'pending') pending
	where earliest and next_attempt_at <= now() and (leased_until is null or leased_until < now())`

const leaseColumns = `leased_by = $3, leased_until = now() + make_interval(secs => $4), attempts = delivery.attempts + 1, last_attempt_at = now()`

const claimEach = `with chosen as (
		select transition_id from (
			select transition_id, rank, next_attempt_at,
				row_number() over (partition by kind order by rank desc, next_attempt_at, transition_id) as turn,
				count(*) over (partition by kind) as waiting
			from (` + dueDeliveries + `) due) ranked
		order by rank desc, turn, waiting, next_attempt_at, transition_id
		limit $2)
	update alert_deliveries delivery set lease = gen_random_uuid(), ` + leaseColumns + `
	from chosen where delivery.receiver = $1 and delivery.transition_id = chosen.transition_id and delivery.state = 'pending'
	returning delivery.lease::text, 1, delivery.attempts, delivery.transition_id, delivery.kind, delivery.transition, delivery.severity`

const claimByKind = `with due as (` + dueDeliveries + `),
	chosen as (
		select kind, transition, gen_random_uuid() as lease from due
		group by kind, transition
		order by max(rank) desc, count(*), min(next_attempt_at)
		limit $2),
	leased as (
		update alert_deliveries delivery set lease = chosen.lease, ` + leaseColumns + `
		from due join chosen on chosen.kind = due.kind and chosen.transition = due.transition
		where delivery.receiver = $1 and delivery.transition_id = due.transition_id and delivery.state = 'pending'
		returning delivery.lease, delivery.attempts, delivery.transition_id, delivery.kind, delivery.transition, delivery.severity)
	select lease::text, count(*), max(attempts), min(transition_id), kind, transition,
		(array_agg(severity order by alert_severity_rank(severity) desc))[1]
	from leased group by lease, kind, transition`

func (store *Store) claim(operation context.Context, target *queue, capacity int) ([]group, error) {
	var claimed []group
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		if _, failure := transaction.Exec(operation, `set local enable_nestloop = off`); failure != nil {
			return failure
		}
		if _, failure := transaction.Exec(operation, `insert into alert_receivers (name) values ($1) on conflict (name) do nothing`, target.name); failure != nil {
			return failure
		}
		var allowed int
		failure := transaction.QueryRow(operation, `select floor($2::float8 - extract(epoch from greatest(sends_at, clock_timestamp()) - clock_timestamp())::float8 / $3::float8)::int
			from alert_receivers where name = $1 and (paused_until is null or paused_until <= clock_timestamp()) for update skip locked`,
			target.name, target.burst, target.interval.Seconds()).Scan(&allowed)
		if errors.Is(failure, pgx.ErrNoRows) || (failure == nil && min(allowed, capacity) <= 0) {
			return nil
		}
		if failure != nil {
			return failure
		}
		query := claimEach
		if target.digests {
			query = claimByKind
		}
		rows, failure := transaction.Query(operation, query, target.name, min(allowed, capacity), store.notifier.instance, deliveryLease.Seconds())
		if failure != nil {
			return failure
		}
		claimed, failure = pgx.CollectRows(rows, func(row pgx.CollectableRow) (group, error) {
			var found group
			return found, row.Scan(&found.lease, &found.count, &found.attempts, &found.transitionID, &found.kind, &found.transition, &found.severity)
		})
		if failure != nil || len(claimed) == 0 {
			return failure
		}
		_, failure = transaction.Exec(operation, `update alert_receivers set sends_at = greatest(sends_at, clock_timestamp()) + make_interval(secs => $2) where name = $1`,
			target.name, target.interval.Seconds()*float64(len(claimed)))
		return failure
	})
	return claimed, failure
}

func (store *Store) deliverFrom(operation context.Context, target *queue) {
	busy := make(chan struct{}, queueWorkers)
	finished := make(chan struct{}, 1)
	var running sync.WaitGroup
	defer running.Wait()
	for {
		if free := queueWorkers - len(busy); free > 0 {
			claimed, failure := store.claim(operation, target, free)
			if failure != nil && operation.Err() == nil {
				store.logger.Warn("due alert notifications were not claimed", "receiver", target.name, "error", failure)
			}
			for _, entry := range claimed {
				busy <- struct{}{}
				running.Go(func() {
					defer func() {
						<-busy
						select {
						case finished <- struct{}{}:
						default:
						}
					}()
					store.deliver(operation, target, entry)
				})
			}
		}
		select {
		case <-operation.Done():
			return
		case <-finished:
		case <-time.After(deliveryTick):
		}
	}
}

func decodeSnapshot(snapshot []byte, message *notification) error {
	if failure := json.Unmarshal(snapshot, &message.alert); failure != nil {
		return fmt.Errorf("the alert recorded with the transition is not readable: %w", failure)
	}
	return nil
}

func (store *Store) load(operation context.Context, target *queue, claimed group) (notification, error) {
	message := notification{receiver: target.name, settings: store.notifier.settings, transition: claimed.transition, severity: claimed.severity, key: claimed.lease}
	if claimed.count > 1 {
		return message, store.loadDigest(operation, claimed, &message)
	}
	var snapshot []byte
	failure := store.pool.QueryRow(operation, `select transition.alert, transition.actor, transition.at, delivery.idempotency_key::text, transition.hostname, transition.campaign_name
		from alert_deliveries delivery join alert_transitions transition on transition.id = delivery.transition_id
		where delivery.transition_id = $1 and delivery.receiver = $2`, claimed.transitionID, target.name).
		Scan(&snapshot, &message.actor, &message.at, &message.key, &message.hostname, &message.campaignName)
	if failure != nil {
		return message, failure
	}
	if failure := decodeSnapshot(snapshot, &message); failure != nil {
		return message, failure
	}
	if failure := message.parseDetail(); failure != nil {
		store.logger.Warn("an alert's detail is not a JSON object; its notification carries no evidence", "alert_id", message.alert.ID, "receiver", target.name, "error", failure)
	}
	return message, nil
}

func (store *Store) loadDigest(operation context.Context, claimed group, message *notification) error {
	summary := &digest{count: claimed.count}
	message.alert.Kind = claimed.kind
	message.digest = summary
	if failure := store.pool.QueryRow(operation, `select min(transition.at), max(transition.at),
			case when count(transition.alert ->> 'tenant') = count(*) and count(distinct transition.alert ->> 'tenant') = 1 then min(transition.alert ->> 'tenant') end
		from alert_deliveries delivery join alert_transitions transition on transition.id = delivery.transition_id
		where delivery.lease = $1`, claimed.lease).Scan(&summary.first, &summary.last, &summary.tenant); failure != nil {
		return failure
	}
	message.at = summary.last
	rows, failure := store.pool.Query(operation, `select transition.alert, transition.hostname, transition.campaign_name
		from alert_deliveries delivery join alert_transitions transition on transition.id = delivery.transition_id
		where delivery.lease = $1 order by alert_severity_rank(transition.severity) desc, transition.id limit $2`, claimed.lease, digestExamples)
	if failure != nil {
		return failure
	}
	summary.examples, failure = pgx.CollectRows(rows, func(row pgx.CollectableRow) (notification, error) {
		example := notification{settings: message.settings}
		var snapshot []byte
		if failure := row.Scan(&snapshot, &example.hostname, &example.campaignName); failure != nil {
			return example, failure
		}
		if failure := decodeSnapshot(snapshot, &example); failure != nil {
			return example, failure
		}
		_ = example.parseDetail()
		return example, nil
	})
	return failure
}

func (store *Store) deliver(operation context.Context, target *queue, claimed group) {
	message, failure := store.load(operation, target, claimed)
	heldBack := false
	if failure == nil {
		failure = store.pool.QueryRow(operation, `select coalesce(paused_until > now(), false) from alert_receivers where name = $1`, target.name).Scan(&heldBack)
	}
	if failure == nil && !heldBack {
		attempt, cancel := context.WithTimeout(operation, deliveryLease/2)
		failure = target.sender.send(attempt, message)
		cancel()
	}
	recording, cancel := context.WithTimeout(context.WithoutCancel(operation), recordTimeout)
	defer cancel()
	fields := []any{"receiver", target.name, "kind", claimed.kind, "transition", claimed.transition, "attempts", claimed.attempts}
	if claimed.count == 1 {
		fields = append(fields, "alert_id", message.alert.ID)
	}
	if heldBack {
		if _, released := store.pool.Exec(recording, `update alert_deliveries set lease = null, leased_by = null, leased_until = null, attempts = attempts - 1
			where lease = $1`, claimed.lease); released != nil {
			store.logger.Warn("an alert notification held back by its receiver's pause was not released; it is retried when its lease ends", append(fields, "error", released)...)
		} else {
			store.logger.Info("alert notification held back until its receiver's pause ends", fields...)
		}
		return
	}
	if operation.Err() != nil {
		if _, released := store.pool.Exec(recording, `update alert_deliveries set lease = null, leased_by = null, leased_until = null, next_attempt_at = now()
			where lease = $1`, claimed.lease); released != nil {
			store.logger.Warn("an alert notification interrupted by shutdown was not released; it is retried when its lease ends", append(fields, "error", released)...)
		}
		return
	}
	var deliveryFailure deliveryError
	var rows pgx.Rows
	var record error
	var delay time.Duration
	switch {
	case failure == nil:
		rows, record = store.pool.Query(recording, `update alert_deliveries set state = 'delivered', delivered_at = now(), last_error = null,
			lease = null, leased_by = null, leased_until = null where lease = $1 returning state`, claimed.lease)
	case errors.As(failure, &deliveryFailure) && deliveryFailure.permanent:
		fields = append(fields, "error", deliveryFailure.message)
		rows, record = store.pool.Query(recording, `update alert_deliveries set state = 'failed', last_error = $2, lease = null, leased_by = null, leased_until = null
			where lease = $1 returning state`, claimed.lease, deliveryFailure.message)
	default:
		delay = max(backoff(claimed.attempts), deliveryFailure.after)
		reason := printable(failure.Error())
		fields = append(fields, "error", reason, "retry_in_seconds", int(delay.Seconds()))
		rows, record = store.pool.Query(recording, `update alert_deliveries set last_error = $2, lease = null, leased_by = null, leased_until = null,
				next_attempt_at = now() + make_interval(secs => $3),
				state = case when created_at + make_interval(secs => $4) <= now() + make_interval(secs => $3) then 'failed' else 'pending' end
			where lease = $1 returning state`, claimed.lease, reason, delay.Seconds(), store.notifier.horizon.Seconds())
	}
	outcomes := map[string]int{}
	if record == nil {
		var state string
		_, record = pgx.ForEachRow(rows, []any{&state}, func() error {
			outcomes[state]++
			return nil
		})
	}
	if deliveryFailure.limited && record == nil {
		if _, paused := store.pool.Exec(recording, `update alert_receivers set paused_until = greatest(paused_until, now() + make_interval(secs => $2)) where name = $1`,
			target.name, delay.Seconds()); paused != nil {
			store.logger.Warn("a receiver that answered 429 was not paused", "receiver", target.name, "error", paused)
		} else {
			store.logger.Warn("alert receiver answered 429; pausing it", "receiver", target.name, "pause_seconds", int(delay.Seconds()))
		}
	}
	switch {
	case record != nil:
		store.logger.Warn("an alert notification's outcome was not recorded; it is retried when its lease ends", append(fields, "alerts", claimed.count, "record_error", record)...)
		return
	case len(outcomes) == 0:
		store.logger.Warn("an alert notification's lease ended before its outcome was recorded; another instance retries it", append(fields, "alerts", claimed.count)...)
		return
	}
	for state, count := range outcomes {
		described := append(slices.Clone(fields), "alerts", count)
		switch state {
		case "delivered":
			deliveries.WithLabelValues(target.name, claimed.transition, "delivered").Add(float64(count))
			store.logger.Info("alert notification delivered", described...)
		case "failed":
			deliveries.WithLabelValues(target.name, claimed.transition, "failed").Add(float64(count))
			store.logger.Error("alert notification gave up", described...)
		default:
			deliveries.WithLabelValues(target.name, claimed.transition, "retry").Add(float64(count))
			store.logger.Warn("alert notification failed; retrying", described...)
		}
	}
}

func (store *Store) expire(operation context.Context) error {
	rows, failure := store.pool.Query(operation, `update alert_deliveries set state = 'failed', lease = null, leased_by = null, leased_until = null,
			last_error = concat_ws('; ', last_error, 'not delivered within alerts.delivery_horizon_seconds')
		where state = 'pending' and created_at <= now() - make_interval(secs => $1) and (leased_until is null or leased_until < now())
		returning alert_id, transition, receiver, attempts`, store.notifier.horizon.Seconds())
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

func (store *Store) failRemoved(operation context.Context) error {
	rows, failure := store.pool.Query(operation, `with failed as (
			update alert_deliveries set state = 'failed', last_error = $2, lease = null, leased_by = null, leased_until = null
			where state = 'pending' and receiver <> all($1::text[])
			returning receiver, transition)
		select receiver, transition, count(*) from failed group by receiver, transition`, append([]string{}, store.notifier.names...), removedReceiver)
	if failure != nil {
		return failure
	}
	var receiver, transition string
	var count int64
	_, failure = pgx.ForEachRow(rows, []any{&receiver, &transition, &count}, func() error {
		deliveries.WithLabelValues(receiver, transition, "failed").Add(float64(count))
		store.logger.Error("alert notification gave up", "receiver", receiver, "transition", transition, "alerts", count, "error", removedReceiver)
		return nil
	})
	return failure
}
