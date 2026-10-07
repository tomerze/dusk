package reconcile

import (
	"reflect"
	"testing"
	"time"

	"dusk/services/twilight/internal/campaign"
)

func TestTheIntendedProcessCacheKeepsIntentsAndAbsences(test *testing.T) {
	cache := newIntendedCache(100)
	identifier := "0192f3a4-5b6c-7d8e-9f01-23456789abcd"
	intended := &IntendedProcess{Pid: pidValue, CampaignID: &identifier, DeviceID: fixtureDevice, InstallationID: fixtureInstallation, ActionKind: "run_script",
		ExpiresAt: start.Add(time.Hour), LastDispatchedAt: start, MaxCommands: 24}
	cache.put(pidValue, intended)
	got, cached := cache.get(pidValue)
	if !cached || !reflect.DeepEqual(got, intended) {
		test.Fatalf("got %+v, want %+v", got, intended)
	}
	absent := campaign.Pid(65536)
	cache.put(absent, nil)
	if got, cached := cache.get(absent); !cached || got != nil {
		test.Fatalf("an absent intended process: %+v %v", got, cached)
	}
	if _, cached := cache.get(65537); cached {
		test.Fatal("a pid never put was cached")
	}
	facts := &IntendedProcess{Pid: 65538, DeviceID: fixtureDevice, InstallationID: fixtureInstallation, ActionKind: "collect_facts", ExpiresAt: start.Add(time.Hour), LastDispatchedAt: start, MaxCommands: 8}
	cache.put(65538, facts)
	if got, cached := cache.get(65538); !cached || got.ActionKind != "collect_facts" || got.CampaignID != nil {
		test.Fatalf("a facts process: %+v %v", got, cached)
	}
	strange := *intended
	strange.ActionKind = "teleport"
	cache.put(65539, &strange)
	if _, cached := cache.get(65539); cached {
		test.Fatal("an intended process of an unknown kind was cached")
	}
}

func TestTheIntendedProcessCacheIsBounded(test *testing.T) {
	cache := newIntendedCache(10)
	for index := range 1000 {
		cache.put(campaign.Pid(65536+index), nil)
	}
	if size := cache.size(); size > 10 {
		test.Fatalf("the cache holds %d entries over its limit of 10", size)
	}
	if _, cached := cache.get(campaign.Pid(65536 + 999)); !cached {
		test.Fatal("the most recent entry was evicted")
	}
}
