//go:build integration

package engine_test

import (
	"context"
	"fmt"
	"slices"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/engine"
)

func (current *harness) runScript(service *engine.Service, name string) (uuid.UUID, campaign.Pid) {
	current.test.Helper()
	definition := campaign.Definition{
		Name:     name,
		Selector: `has(device_id)`,
		Action:   campaign.Action{Kind: campaign.KindRunScript, Script: "ps"},
		Policy: campaign.Policy{
			Rate:               campaign.Rate{PerSecond: 50, Burst: 50},
			Phases:             []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}},
			NodeTimeoutSeconds: 1,
			Gates:              campaign.Gates{MinSample: 1},
		},
	}
	created, failure := service.CreateCampaign(context.Background(), definition, "operator@example.org")
	if failure != nil {
		current.test.Fatal(failure)
	}
	if _, failure := service.StartCampaign(context.Background(), created.ID, "operator@example.org"); failure != nil {
		current.test.Fatal(failure)
	}
	var pid string
	current.eventually(name+" succeeds on the node", 90*time.Second, func() (bool, string) {
		failure := current.pool.QueryRow(context.Background(), `select pid::text from campaign_nodes where campaign_id = $1 and state = 'succeeded'`, created.ID).Scan(&pid)
		return failure == nil, fmt.Sprintf("%v %v", current.rows(created.ID), failure)
	})
	parsed, failure := campaign.ParsePid(pid)
	if failure != nil {
		current.test.Fatal(failure)
	}
	return created.ID, parsed
}

func (current *harness) pastHorizon(identifier uuid.UUID) {
	current.test.Helper()
	if _, failure := current.pool.Exec(context.Background(), `update intended_processes set expires_at = now() - interval '1 day' where campaign_id = $1`, identifier); failure != nil {
		current.test.Fatal(failure)
	}
}

func (current *harness) reaped(device string) []campaign.Pid {
	current.fake.mutex.Lock()
	defer current.fake.mutex.Unlock()
	return append([]campaign.Pid(nil), current.fake.reaps[device]...)
}

func (current *harness) recordedReaped(identifier uuid.UUID, pid campaign.Pid) bool {
	var row, intended bool
	_ = current.pool.QueryRow(context.Background(), `select
		exists (select 1 from campaign_nodes where campaign_id = $1 and pid = $2::text::numeric and reaped_at is not null),
		exists (select 1 from intended_processes where campaign_id = $1 and pid = $2::text::numeric and reaped_at is not null)`,
		identifier, pid.String()).Scan(&row, &intended)
	return row && intended
}

func TestEngineReapsPidsPastTheirRetryHorizon(test *testing.T) {
	current := newHarness(test)
	current.settings.Engine.ReapIntervalSeconds = 30
	target := identity(31)
	copied := target
	current.fake.nodes[target.device] = &copied
	current.enroll(target)
	current.census(target)
	current.start()
	service := current.engine.Service()
	current.eventually("the node is online with its facts", time.Minute, func() (bool, string) {
		var read int
		_ = current.pool.QueryRow(context.Background(), `select count(*) from nodes where facts_read_at is not null`).Scan(&read)
		return current.engine.View().OnlineCount() == 1 && read == 1, fmt.Sprintf("online %d facts %d", current.engine.View().OnlineCount(), read)
	})

	first, firstPid := current.runScript(service, "first")
	second, secondPid := current.runScript(service, "second")
	if firstPid == secondPid || firstPid != campaign.DerivePid(first, target.device, target.installation, 1) {
		test.Fatalf("pids %s and %s", firstPid, secondPid)
	}
	time.Sleep(3 * time.Second)
	if early := current.reaped(target.device); len(early) != 0 {
		test.Fatalf("pids were reaped before their retry horizon passed: %v", early)
	}

	current.pastHorizon(first)
	current.eventually("the sweep reaps the first campaign's pid on the online node", time.Minute, func() (bool, string) {
		return slices.Contains(current.reaped(target.device), firstPid) && current.recordedReaped(first, firstPid), fmt.Sprint(current.reaped(target.device))
	})
	if slices.Contains(current.reaped(target.device), secondPid) {
		test.Fatal("a pid inside its retry horizon was reaped")
	}

	current.pastHorizon(second)
	target.epoch++
	current.connect(target, "connected")
	current.eventually("a reconnect reaps the second campaign's pid", time.Minute, func() (bool, string) {
		return slices.Contains(current.reaped(target.device), secondPid) && current.recordedReaped(second, secondPid), fmt.Sprint(current.reaped(target.device))
	})
	asked := len(current.reaped(target.device))
	target.epoch++
	current.connect(target, "connected")
	time.Sleep(3 * time.Second)
	if later := current.reaped(target.device); len(later) != asked {
		test.Fatalf("pids dawn reported reaped were asked again: %v", later)
	}
	var requests, budgeted int
	_ = current.pool.QueryRow(context.Background(), `select count(*), count(*) filter (where max_commands = 0 and default_shell_commands = 4)
		from intended_processes where device_id = $1 and action_kind = 'reap'`, target.device).Scan(&requests, &budgeted)
	if requests < 2 || budgeted != requests {
		test.Fatalf("%d reap requests were recorded as intended processes, %d with the default shell's reap budget", requests, budgeted)
	}
}
