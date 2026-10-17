//go:build integration

package engine_test

import (
	"context"
	"fmt"
	"maps"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
)

func TestARowInFlightWhenItsCampaignFinishesIsCountedWhereItEnds(test *testing.T) {
	current := newHarness(test)
	current.start()
	service := current.engine.Service()
	current.eventually("the instance owns every slice", 2*time.Minute, func() (bool, string) {
		var owned int
		_ = current.pool.QueryRow(context.Background(), `select count(*) from node_partition_leases where owner = $1`, current.settings.Instance).Scan(&owned)
		return owned == 3, fmt.Sprintf("%d slices owned", owned)
	})
	script := campaign.Definition{
		Name:     "aborted with a process in flight",
		Selector: `device_id == "none"`,
		Action:   campaign.Action{Kind: campaign.KindRunScript, Script: "ps"},
		Policy: campaign.Policy{Rate: campaign.Rate{PerSecond: 50, Burst: 50}, Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}},
			NodeTimeoutSeconds: 5, Gates: campaign.Gates{MinSample: 1}},
	}
	created, failure := service.CreateCampaign(context.Background(), script, "operator@example.org")
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := service.StartCampaign(context.Background(), created.ID, "operator@example.org"); failure != nil {
		test.Fatal(failure)
	}
	target := identity(320)
	if _, failure := current.pool.Exec(context.Background(), `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, attempt, pid, dispatched_at, deadline_at)
		values ($1, $2, $3, 0, 'dispatched', 1, $4::text::numeric, now(), now() + interval '1 hour')`,
		created.ID, target.device, target.installation, campaign.DerivePid(created.ID, target.device, target.installation, 1).String()); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := service.AbortCampaign(context.Background(), created.ID, "operator@example.org", "test"); failure != nil {
		test.Fatal(failure)
	}
	store := campaign.NewStore(current.pool)
	counted := func() map[campaign.NodeState]int {
		summed, failure := store.Counters(context.Background(), []uuid.UUID{created.ID})
		if failure != nil {
			test.Fatal(failure)
		}
		totals := map[campaign.NodeState]int{}
		for _, counter := range summed[created.ID] {
			totals[counter.State] += int(counter.Count)
		}
		return totals
	}
	if totals := counted(); totals[campaign.StateDispatched] != 1 {
		test.Fatalf("the abort counted %v; want the row still in flight as dispatched", totals)
	}
	if _, failure := current.pool.Exec(context.Background(), `update campaign_nodes set deadline_at = now() - interval '1 second' where campaign_id = $1`, created.ID); failure != nil {
		test.Fatal(failure)
	}
	current.eventually("the row's deadline moves it and its partition is counted again", time.Minute, func() (bool, string) {
		rows, totals := current.rows(created.ID), counted()
		return rows[campaign.StateDispatched] == 0 && maps.Equal(rows, totals), fmt.Sprintf("rows %v, counters %v", rows, totals)
	})
}
