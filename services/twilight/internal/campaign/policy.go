package campaign

import (
	"encoding/json"
	"math"
	"math/big"
	"strconv"
	"strings"
	"time"
)

type Rate struct {
	PerSecond float64 `json:"per_second"`
	Burst     int     `json:"burst"`
}

type Phase struct {
	Name        string  `json:"name"`
	Percent     float64 `json:"percent"`
	BakeSeconds int     `json:"bake_seconds"`
}

type Gates struct {
	MinSample           int      `json:"min_sample"`
	MaxFailureRate      float64  `json:"max_failure_rate"`
	MaxSilentRate       float64  `json:"max_silent_rate"`
	SilentWindowSeconds int      `json:"silent_window_seconds"`
	Breakdown           []string `json:"breakdown"`
}

type GateAction string

const (
	GateActionPause GateAction = "pause"
	GateActionAbort GateAction = "abort"
)

type Abort struct {
	OnGateFailure    GateAction `json:"on_gate_failure"`
	MaxTotalFailures *int       `json:"max_total_failures"`
}

type Retry struct {
	MaxAttempts           int     `json:"max_attempts"`
	InitialBackoffSeconds int     `json:"initial_backoff_seconds"`
	MaxBackoffSeconds     int     `json:"max_backoff_seconds"`
	Multiplier            float64 `json:"multiplier"`
}

type Policy struct {
	Rate               Rate       `json:"rate"`
	Phases             []Phase    `json:"phases"`
	Gates              Gates      `json:"gates"`
	Abort              Abort      `json:"abort"`
	NodeTimeoutSeconds int        `json:"node_timeout_seconds"`
	Retry              Retry      `json:"retry"`
	Deadline           *time.Time `json:"deadline"`
	AllowOverlap       bool       `json:"allow_overlap"`
}

var BreakdownDimensions = []string{"os_build", "hardware_class", "dusk_version", "country"}

func DefaultGates() Gates {
	return Gates{MinSample: 1, MaxFailureRate: 0.05, MaxSilentRate: 0.05, SilentWindowSeconds: 600, Breakdown: append([]string(nil), BreakdownDimensions...)}
}

func (gates *Gates) UnmarshalJSON(document []byte) error {
	type plain Gates
	decoded := plain(DefaultGates())
	if failure := json.Unmarshal(document, &decoded); failure != nil {
		return failure
	}
	*gates = Gates(decoded)
	return nil
}

func (policy *Policy) UnmarshalJSON(document []byte) error {
	type plain Policy
	decoded := plain{Gates: DefaultGates()}
	if failure := json.Unmarshal(document, &decoded); failure != nil {
		return failure
	}
	*policy = Policy(decoded)
	return nil
}

func (policy *Policy) ApplyDefaults(kind Kind) {
	if policy.Rate.PerSecond == 0 {
		policy.Rate.PerSecond = 10
	}
	if policy.Rate.Burst == 0 {
		policy.Rate.Burst = max(1, int(math.Ceil(policy.Rate.PerSecond)))
	}
	if policy.NodeTimeoutSeconds == 0 {
		policy.NodeTimeoutSeconds = 900
	}
	if len(policy.Phases) == 0 {
		policy.Phases = []Phase{{Name: "all", Percent: 100, BakeSeconds: max(policy.NodeTimeoutSeconds, policy.Gates.SilentWindowSeconds)}}
	}
	if policy.Gates.Breakdown == nil {
		policy.Gates.Breakdown = append([]string(nil), BreakdownDimensions...)
	}
	if policy.Abort.OnGateFailure == "" {
		policy.Abort.OnGateFailure = GateActionPause
	}
	if policy.Retry.MaxAttempts == 0 {
		policy.Retry.MaxAttempts = 3
		if kind.OneShot() {
			policy.Retry.MaxAttempts = 1
		}
	}
	if policy.Retry.InitialBackoffSeconds == 0 {
		policy.Retry.InitialBackoffSeconds = 60
	}
	if policy.Retry.MaxBackoffSeconds == 0 {
		policy.Retry.MaxBackoffSeconds = max(3600, policy.Retry.InitialBackoffSeconds)
	}
	if policy.Retry.Multiplier == 0 {
		policy.Retry.Multiplier = 2
	}
}

func (policy Policy) Validate(now time.Time) error {
	if math.IsNaN(policy.Rate.PerSecond) || policy.Rate.PerSecond <= 0 || policy.Rate.PerSecond > 100000 {
		return invalid("policy.rate.per_second", "must be above 0 and at most 100000")
	}
	if policy.Rate.Burst < 1 || policy.Rate.Burst > 1000000 {
		return invalid("policy.rate.burst", "must be between 1 and 1000000")
	}
	if len(policy.Phases) == 0 || len(policy.Phases) > 20 {
		return invalid("policy.phases", "must hold between 1 and 20 phases")
	}
	if policy.NodeTimeoutSeconds < 1 || policy.NodeTimeoutSeconds > 7*86400 {
		return invalid("policy.node_timeout_seconds", "must be between 1 and 604800")
	}
	gates := policy.Gates
	if gates.MinSample < 1 || gates.MinSample > 10000000 {
		return invalid("policy.gates.min_sample", "must be between 1 and 10000000")
	}
	if math.IsNaN(gates.MaxFailureRate) || gates.MaxFailureRate < 0 || gates.MaxFailureRate > 1 {
		return invalid("policy.gates.max_failure_rate", "must be between 0 and 1")
	}
	if math.IsNaN(gates.MaxSilentRate) || gates.MaxSilentRate < 0 || gates.MaxSilentRate > 1 {
		return invalid("policy.gates.max_silent_rate", "must be between 0 and 1")
	}
	if gates.SilentWindowSeconds < 0 || gates.SilentWindowSeconds > 7*86400 {
		return invalid("policy.gates.silent_window_seconds", "must be between 0 and 604800")
	}
	allowed := map[string]bool{}
	for _, dimension := range BreakdownDimensions {
		allowed[dimension] = true
	}
	seenDimensions := map[string]bool{}
	for _, dimension := range gates.Breakdown {
		if !allowed[dimension] {
			return invalid("policy.gates.breakdown", "%q is not one of %s", dimension, strings.Join(BreakdownDimensions, ", "))
		}
		if seenDimensions[dimension] {
			return invalid("policy.gates.breakdown", "lists %q twice", dimension)
		}
		seenDimensions[dimension] = true
	}
	previous := 0.0
	names := map[string]bool{}
	for index, phase := range policy.Phases {
		field := "policy.phases[" + strconv.Itoa(index) + "]"
		name := strings.TrimSpace(phase.Name)
		if name == "" || len(name) > 100 {
			return invalid(field+".name", "is required and at most 100 characters")
		}
		if names[name] {
			return invalid(field+".name", "%q is used by an earlier phase", name)
		}
		names[name] = true
		if math.IsNaN(phase.Percent) || phase.Percent <= previous || phase.Percent > 100 {
			return invalid(field+".percent", "must be above the previous phase's %g and at most 100", previous)
		}
		if math.Round(phase.Percent*1000) != phase.Percent*1000 {
			return invalid(field+".percent", "has more than three decimal places")
		}
		previous = phase.Percent
		if phase.BakeSeconds < 0 || phase.BakeSeconds > 30*86400 {
			return invalid(field+".bake_seconds", "must be between 0 and 2592000")
		}
		if phase.BakeSeconds < gates.SilentWindowSeconds {
			return invalid(field+".bake_seconds", "must be at least gates.silent_window_seconds (%d)", gates.SilentWindowSeconds)
		}
		if phase.BakeSeconds < policy.NodeTimeoutSeconds {
			return invalid(field+".bake_seconds", "must be at least node_timeout_seconds (%d)", policy.NodeTimeoutSeconds)
		}
	}
	if previous != 100 {
		return invalid("policy.phases", "the last phase must reach 100 percent")
	}
	if policy.Abort.OnGateFailure != GateActionPause && policy.Abort.OnGateFailure != GateActionAbort {
		return invalid("policy.abort.on_gate_failure", "must be pause or abort")
	}
	if policy.Abort.MaxTotalFailures != nil && *policy.Abort.MaxTotalFailures < 1 {
		return invalid("policy.abort.max_total_failures", "must be at least 1 or null")
	}
	retry := policy.Retry
	if retry.MaxAttempts < 1 || retry.MaxAttempts > 100 {
		return invalid("policy.retry.max_attempts", "must be between 1 and 100")
	}
	if retry.InitialBackoffSeconds < 1 || retry.InitialBackoffSeconds > 86400 {
		return invalid("policy.retry.initial_backoff_seconds", "must be between 1 and 86400")
	}
	if retry.MaxBackoffSeconds < retry.InitialBackoffSeconds || retry.MaxBackoffSeconds > 7*86400 {
		return invalid("policy.retry.max_backoff_seconds", "must be between initial_backoff_seconds and 604800")
	}
	if math.IsNaN(retry.Multiplier) || retry.Multiplier < 1 || retry.Multiplier > 10 {
		return invalid("policy.retry.multiplier", "must be between 1 and 10")
	}
	if policy.Deadline != nil && !policy.Deadline.After(now) {
		return invalid("policy.deadline", "is in the past")
	}
	return nil
}

func (policy Policy) MaxBakeSeconds() int {
	longest := 0
	for _, phase := range policy.Phases {
		longest = max(longest, phase.BakeSeconds)
	}
	return longest
}

func (policy Policy) Backoff(failures int, random float64) time.Duration {
	delay := float64(policy.Retry.InitialBackoffSeconds) * math.Pow(policy.Retry.Multiplier, float64(max(0, failures-1)))
	delay = math.Min(delay, float64(policy.Retry.MaxBackoffSeconds))
	return time.Duration(random * delay * float64(time.Second))
}

var twoToThe64 = new(big.Int).Lsh(big.NewInt(1), 64)

type PhaseThresholds []*big.Int

func (policy Policy) Thresholds() PhaseThresholds {
	thresholds := make(PhaseThresholds, len(policy.Phases))
	for index, phase := range policy.Phases {
		percent, _ := new(big.Rat).SetString(strconv.FormatFloat(phase.Percent, 'f', -1, 64))
		scaled := new(big.Rat).Mul(percent, new(big.Rat).SetInt(twoToThe64))
		scaled.Quo(scaled, big.NewRat(100, 1))
		ceiling := new(big.Int).Quo(scaled.Num(), scaled.Denom())
		if new(big.Int).Mul(ceiling, scaled.Denom()).Cmp(scaled.Num()) != 0 {
			ceiling.Add(ceiling, big.NewInt(1))
		}
		thresholds[index] = ceiling
	}
	return thresholds
}

func (thresholds PhaseThresholds) PhaseOf(bucket uint64) int {
	value := new(big.Int).SetUint64(bucket)
	for index, threshold := range thresholds {
		if value.Cmp(threshold) < 0 {
			return index
		}
	}
	return len(thresholds) - 1
}

func (thresholds PhaseThresholds) Open(bucket uint64, currentPhase int) bool {
	if currentPhase < 0 || currentPhase >= len(thresholds) {
		return false
	}
	return new(big.Int).SetUint64(bucket).Cmp(thresholds[currentPhase]) < 0
}
