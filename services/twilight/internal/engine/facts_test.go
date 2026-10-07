package engine

import (
	"slices"
	"testing"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
)

func TestFactsReadsAskForTheKeysOfOpenEnsureVersionRows(test *testing.T) {
	application, agent, finished := uuid.New(), uuid.New(), uuid.New()
	keys := map[uuid.UUID]string{application: "app.version", agent: "agent.version", finished: "old.version"}
	rows := []campaign.Row{
		{CampaignID: application, State: campaign.StateVerifying},
		{CampaignID: agent, State: campaign.StatePending},
		{CampaignID: finished, State: campaign.StateSucceeded},
		{CampaignID: uuid.New(), State: campaign.StateVerifying},
	}
	found := versionKeys(keys, rows)
	if !slices.Equal(found, []string{"agent.version", "app.version"}) {
		test.Fatalf("version keys %v", found)
	}
	if found := versionKeys(keys, nil); found == nil || len(found) != 0 {
		test.Fatalf("a node with no open row asks for %v", found)
	}
	for _, check := range []struct {
		keys     []string
		commands int
	}{{nil, 2}, {[]string{"app.version"}, 3}, {[]string{"dusk.config.hash", "app.version", "agent.version"}, 4}} {
		if got := factsCommands(check.keys); got != check.commands {
			test.Errorf("a facts read of %v may run %d commands, want %d", check.keys, got, check.commands)
		}
	}
}
