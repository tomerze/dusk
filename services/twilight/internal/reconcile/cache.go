package reconcile

import (
	"encoding/hex"
	"sync"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
)

var actionKinds = []string{"run_script", "ensure_version", "ensure_config", "quarantine", "collect_facts", "collect_file", "stream_logs", "interactive"}

type compactIntended struct {
	missing          bool
	hasCampaign      bool
	kind             uint8
	maxCommands      int32
	device           [16]byte
	installation     [16]byte
	campaign         [16]byte
	expiresAt        int64
	lastDispatchedAt int64
}

type intendedCache struct {
	mutex sync.Mutex
	limit int
	hot   map[campaign.Pid]compactIntended
	cold  map[campaign.Pid]compactIntended
}

func newIntendedCache(limit int) *intendedCache {
	return &intendedCache{limit: max(limit, 2), hot: map[campaign.Pid]compactIntended{}, cold: map[campaign.Pid]compactIntended{}}
}

func compact(intended *IntendedProcess) (compactIntended, bool) {
	if intended == nil {
		return compactIntended{missing: true}, true
	}
	var packed compactIntended
	kind := -1
	for index, name := range actionKinds {
		if name == intended.ActionKind {
			kind = index
		}
	}
	device, deviceFailure := hex.DecodeString(intended.DeviceID)
	installation, installationFailure := hex.DecodeString(intended.InstallationID)
	if kind < 0 || deviceFailure != nil || installationFailure != nil || len(device) != 16 || len(installation) != 16 ||
		intended.MaxCommands < 1 || intended.MaxCommands > 1<<30 {
		return packed, false
	}
	packed.kind = uint8(kind)
	copy(packed.device[:], device)
	copy(packed.installation[:], installation)
	if intended.CampaignID != nil {
		identifier, failure := uuid.Parse(*intended.CampaignID)
		if failure != nil {
			return packed, false
		}
		packed.hasCampaign, packed.campaign = true, identifier
	}
	packed.maxCommands = int32(intended.MaxCommands)
	packed.expiresAt = intended.ExpiresAt.UnixNano()
	packed.lastDispatchedAt = intended.LastDispatchedAt.UnixNano()
	return packed, true
}

func (packed compactIntended) expand(pid campaign.Pid) *IntendedProcess {
	if packed.missing {
		return nil
	}
	intended := &IntendedProcess{
		Pid: pid, DeviceID: hex.EncodeToString(packed.device[:]), InstallationID: hex.EncodeToString(packed.installation[:]),
		ActionKind: actionKinds[packed.kind], ExpiresAt: time.Unix(0, packed.expiresAt).UTC(),
		LastDispatchedAt: time.Unix(0, packed.lastDispatchedAt).UTC(), MaxCommands: int(packed.maxCommands),
	}
	if packed.hasCampaign {
		identifier := uuid.UUID(packed.campaign).String()
		intended.CampaignID = &identifier
	}
	return intended
}

func (cache *intendedCache) get(pid campaign.Pid) (*IntendedProcess, bool) {
	cache.mutex.Lock()
	defer cache.mutex.Unlock()
	if packed, found := cache.hot[pid]; found {
		return packed.expand(pid), true
	}
	if packed, found := cache.cold[pid]; found {
		cache.store(pid, packed)
		return packed.expand(pid), true
	}
	return nil, false
}

func (cache *intendedCache) put(pid campaign.Pid, intended *IntendedProcess) {
	packed, representable := compact(intended)
	if !representable {
		return
	}
	cache.mutex.Lock()
	defer cache.mutex.Unlock()
	cache.store(pid, packed)
}

func (cache *intendedCache) store(pid campaign.Pid, packed compactIntended) {
	if len(cache.hot) >= cache.limit/2 {
		cache.cold = cache.hot
		cache.hot = map[campaign.Pid]compactIntended{}
	}
	cache.hot[pid] = packed
}

func (cache *intendedCache) size() int {
	cache.mutex.Lock()
	defer cache.mutex.Unlock()
	return len(cache.hot) + len(cache.cold)
}
