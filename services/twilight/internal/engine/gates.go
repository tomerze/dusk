package engine

import (
	"context"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"

	"dusk/services/twilight/internal/campaign"
)

type Tally struct {
	Succeeded int64 `json:"succeeded"`
	Failed    int64 `json:"failed"`
	Eligible  int64 `json:"eligible"`
	Silent    int64 `json:"silent"`
}

func (tally Tally) FailureRate() float64 {
	if tally.Succeeded+tally.Failed == 0 {
		return 0
	}
	return float64(tally.Failed) / float64(tally.Succeeded+tally.Failed)
}

func (tally Tally) SilentRate() float64 {
	if tally.Eligible == 0 {
		return 0
	}
	return float64(tally.Silent) / float64(tally.Eligible)
}

const (
	VerdictPass = "pass"
	VerdictHold = "hold"
	VerdictFail = "fail"
)

type GateReport struct {
	Overall        Tally                       `json:"overall"`
	Groups         map[string]map[string]Tally `json:"groups"`
	Verdict        string                      `json:"verdict"`
	Reason         string                      `json:"reason"`
	FailingGroup   string                      `json:"failing_group"`
	Degraded       bool                        `json:"degraded"`
	MinSample      int                         `json:"min_sample"`
	Required       int64                       `json:"required"`
	MaxFailureRate float64                     `json:"max_failure_rate"`
	MaxSilentRate  float64                     `json:"max_silent_rate"`
}

type breach struct {
	group  string
	reason string
	rate   float64
}

func Judge(gates campaign.Gates, maxTotalFailures *int, overall Tally, groups map[string]map[string]Tally, rows int64, degraded bool) GateReport {
	minimum := int64(gates.MinSample)
	required := min(minimum, rows)
	report := GateReport{Overall: overall, Groups: groups, Degraded: degraded, MinSample: gates.MinSample, Required: required, MaxFailureRate: gates.MaxFailureRate, MaxSilentRate: gates.MaxSilentRate}
	var breaches []breach
	check := func(group string, tally Tally, needed int64) {
		where := "overall"
		if group != "" {
			where = "in " + group
		}
		if sample := tally.Succeeded + tally.Failed; sample >= needed && sample > 0 && tally.FailureRate() > gates.MaxFailureRate {
			breaches = append(breaches, breach{group, fmt.Sprintf("failure rate %.2f %s, %d of %d", tally.FailureRate(), where, tally.Failed, sample), tally.FailureRate()})
		}
		if !degraded && tally.Eligible >= needed && tally.Eligible > 0 && tally.SilentRate() > gates.MaxSilentRate {
			breaches = append(breaches, breach{group, fmt.Sprintf("silent rate %.2f %s, %d of %d", tally.SilentRate(), where, tally.Silent, tally.Eligible), tally.SilentRate()})
		}
	}
	check("", overall, required)
	dimensions := make([]string, 0, len(groups))
	for dimension := range groups {
		dimensions = append(dimensions, dimension)
	}
	sort.Strings(dimensions)
	for _, dimension := range dimensions {
		values := make([]string, 0, len(groups[dimension]))
		for value := range groups[dimension] {
			values = append(values, value)
		}
		sort.Strings(values)
		for _, value := range values {
			check(dimension+"="+value, groups[dimension][value], minimum)
		}
	}
	if maxTotalFailures != nil && overall.Failed > int64(*maxTotalFailures) {
		breaches = append(breaches, breach{"", fmt.Sprintf("%d failures, more than max_total_failures %d", overall.Failed, *maxTotalFailures), 2})
	}
	if len(breaches) > 0 {
		worst := breaches[0]
		for _, candidate := range breaches[1:] {
			if candidate.rate > worst.rate {
				worst = candidate
			}
		}
		report.Verdict, report.Reason, report.FailingGroup = VerdictFail, worst.reason, worst.group
		return report
	}
	if sample := overall.Succeeded + overall.Failed; sample < required {
		report.Verdict, report.Reason = VerdictHold, fmt.Sprintf("waiting for sample %d of %d", sample, required)
		return report
	}
	if overall.Eligible < required {
		report.Verdict, report.Reason = VerdictHold, fmt.Sprintf("waiting for silent-window sample %d of %d", overall.Eligible, required)
		return report
	}
	if degraded {
		report.Verdict, report.Reason = VerdictHold, "the online view is degraded"
		return report
	}
	report.Verdict = VerdictPass
	return report
}

var breakdownExpressions = map[string]string{
	"os_build":       "breakdown->>'os_build'",
	"hardware_class": "breakdown->>'hardware_class'",
	"dusk_version":   "breakdown->>'dusk_version'",
	"country":        "breakdown->>'country'",
}

func Tallies(operation context.Context, pool *pgxpool.Pool, target *campaign.Campaign, now time.Time) (Tally, map[string]map[string]Tally, int64, error) {
	window := time.Duration(target.Policy.Gates.SilentWindowSeconds) * time.Second
	var dimensions []string
	for _, dimension := range target.Policy.Gates.Breakdown {
		if _, known := breakdownExpressions[dimension]; known {
			dimensions = append(dimensions, dimension)
		}
	}
	sets := []string{"()"}
	columns := make([]string, len(dimensions))
	for index, dimension := range dimensions {
		expression := breakdownExpressions[dimension]
		sets = append(sets, "(coalesce("+expression+", ''))")
		columns[index] = "coalesce(" + expression + ", '')"
	}
	selectColumns := ""
	for index, column := range columns {
		selectColumns += fmt.Sprintf("%s as dimension_%d, grouping(%s) as grouped_%d, ", column, index, column, index)
	}
	query := `select ` + selectColumns + `
		count(*) filter (where state = 'succeeded'),
		count(*) filter (where state = 'unknown' or (state in ('backoff', 'failed') and last_status in ('failed', 'resolved'))),
		count(*) filter (where state = 'succeeded' and finished_at <= $3),
		count(*) filter (where state = 'succeeded' and finished_at <= $3 and silent),
		(select count(*) from campaign_nodes where campaign_id = $1 and phase <= $2)
		from campaign_nodes where campaign_id = $1 and phase <= $2 and ($4::timestamptz is null or dispatched_at >= $4)
		group by grouping sets (` + strings.Join(sets, ", ") + `)`
	rows, failure := pool.Query(operation, query, target.ID, target.CurrentPhase, now.Add(-window), target.GateOverrideAfter)
	if failure != nil {
		return Tally{}, nil, 0, fmt.Errorf("gate tallies: %w", failure)
	}
	defer rows.Close()
	var overall Tally
	var phaseRows int64
	groups := map[string]map[string]Tally{}
	for rows.Next() {
		values := make([]*string, len(dimensions))
		grouped := make([]int32, len(dimensions))
		targets := make([]any, 0, 2*len(dimensions)+5)
		for index := range dimensions {
			targets = append(targets, &values[index], &grouped[index])
		}
		var tally Tally
		targets = append(targets, &tally.Succeeded, &tally.Failed, &tally.Eligible, &tally.Silent, &phaseRows)
		if failure := rows.Scan(targets...); failure != nil {
			return Tally{}, nil, 0, fmt.Errorf("gate tallies: %w", failure)
		}
		dimension := -1
		for index := range dimensions {
			if grouped[index] == 0 {
				dimension = index
			}
		}
		if dimension < 0 {
			overall = tally
			continue
		}
		name := dimensions[dimension]
		if groups[name] == nil {
			groups[name] = map[string]Tally{}
		}
		value := ""
		if values[dimension] != nil {
			value = *values[dimension]
		}
		groups[name][value] = tally
	}
	return overall, groups, phaseRows, rows.Err()
}
