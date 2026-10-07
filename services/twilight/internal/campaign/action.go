package campaign

import (
	"fmt"
	"path"
	"regexp"
	"strings"
)

type Kind string

const (
	KindRunScript     Kind = "run_script"
	KindEnsureVersion Kind = "ensure_version"
	KindEnsureConfig  Kind = "ensure_config"
	KindQuarantine    Kind = "quarantine"
)

func (kind Kind) Valid() bool {
	switch kind {
	case KindRunScript, KindEnsureVersion, KindEnsureConfig, KindQuarantine:
		return true
	}
	return false
}

func (kind Kind) OneShot() bool {
	return kind == KindRunScript || kind == KindQuarantine
}

func (kind Kind) Converging() bool {
	return kind == KindEnsureVersion || kind == KindEnsureConfig
}

const (
	DefaultVersionKey     = "dusk.version"
	ConfigHashKey         = "dusk.config.hash"
	MaximumScriptBytes    = 262144
	MaximumCollectedFiles = 16
)

type StreamLogs struct {
	Level           string `json:"level"`
	DurationSeconds int    `json:"duration_seconds"`
}

type Action struct {
	Kind                 Kind        `json:"kind"`
	Script               string      `json:"script,omitempty"`
	CollectFiles         []string    `json:"collect_files,omitempty"`
	StreamLogs           *StreamLogs `json:"stream_logs,omitempty"`
	Version              string      `json:"version,omitempty"`
	VersionKey           string      `json:"version_key,omitempty"`
	ConfigHash           string      `json:"config_hash,omitempty"`
	RequireScriptSuccess bool        `json:"require_script_success,omitempty"`
}

type ValidationError struct {
	Field   string `json:"field"`
	Message string `json:"message"`
}

func (failure *ValidationError) Error() string {
	return failure.Field + ": " + failure.Message
}

func invalid(field, format string, arguments ...any) *ValidationError {
	return &ValidationError{Field: field, Message: fmt.Sprintf(format, arguments...)}
}

var (
	factKeyPattern = regexp.MustCompile(`^[A-Za-z0-9_][A-Za-z0-9_.:-]{0,254}$`)
	tenantPattern  = regexp.MustCompile(`^[a-z0-9-]{1,63}$`)
	logLevels      = map[string]bool{"trace": true, "debug": true, "info": true, "warn": true, "error": true}
)

func (action *Action) Normalize() {
	if action.Kind == KindEnsureVersion && action.VersionKey == "" {
		action.VersionKey = DefaultVersionKey
	}
}

func (action Action) Validate() error {
	if !action.Kind.Valid() {
		return invalid("action.kind", "must be one of run_script, ensure_version, ensure_config, quarantine")
	}
	if len(action.Script) > MaximumScriptBytes {
		return invalid("action.script", "is longer than %d bytes", MaximumScriptBytes)
	}
	if strings.ContainsRune(action.Script, 0) {
		return invalid("action.script", "cannot contain U+0000")
	}
	needsScript := action.Kind != KindQuarantine
	if needsScript && strings.TrimSpace(action.Script) == "" {
		return invalid("action.script", "is required for %s", action.Kind)
	}
	if action.Kind != KindRunScript && (len(action.CollectFiles) > 0 || action.StreamLogs != nil) {
		return invalid("action.collect_files", "and stream_logs are only for run_script")
	}
	if len(action.CollectFiles) > MaximumCollectedFiles {
		return invalid("action.collect_files", "holds at most %d paths", MaximumCollectedFiles)
	}
	seen := map[string]bool{}
	for _, file := range action.CollectFiles {
		if file == "" || strings.ContainsRune(file, 0) || len(file) > 4096 {
			return invalid("action.collect_files", "%q is not a usable path", file)
		}
		if !path.IsAbs(file) && !isWindowsAbsolute(file) {
			return invalid("action.collect_files", "%q is not an absolute path", file)
		}
		if seen[file] {
			return invalid("action.collect_files", "lists %q twice", file)
		}
		seen[file] = true
	}
	if action.StreamLogs != nil {
		if !logLevels[action.StreamLogs.Level] {
			return invalid("action.stream_logs.level", "must be one of trace, debug, info, warn, error")
		}
		if action.StreamLogs.DurationSeconds < 1 || action.StreamLogs.DurationSeconds > 86400 {
			return invalid("action.stream_logs.duration_seconds", "must be between 1 and 86400")
		}
	}
	switch action.Kind {
	case KindEnsureVersion:
		if strings.TrimSpace(action.Version) == "" || len(action.Version) > 256 {
			return invalid("action.version", "is required and at most 256 characters")
		}
		if !factKeyPattern.MatchString(action.VersionKey) {
			return invalid("action.version_key", "%q is not a key name", action.VersionKey)
		}
		if action.VersionKey == "dusk.device.id" {
			return invalid("action.version_key", "cannot be dusk.device.id")
		}
	case KindEnsureConfig:
		if strings.TrimSpace(action.ConfigHash) == "" || len(action.ConfigHash) > 256 {
			return invalid("action.config_hash", "is required and at most 256 characters")
		}
	default:
		if action.Version != "" || action.VersionKey != "" || action.ConfigHash != "" {
			return invalid("action", "version, version_key and config_hash belong to ensure_version and ensure_config")
		}
	}
	if action.Kind != KindQuarantine && action.RequireScriptSuccess {
		return invalid("action.require_script_success", "belongs to quarantine")
	}
	if action.Kind == KindQuarantine && action.RequireScriptSuccess && strings.TrimSpace(action.Script) == "" {
		return invalid("action.require_script_success", "needs a script")
	}
	return nil
}

func isWindowsAbsolute(file string) bool {
	return len(file) >= 3 && ((file[0] >= 'A' && file[0] <= 'Z') || (file[0] >= 'a' && file[0] <= 'z')) && file[1] == ':' && (file[2] == '\\' || file[2] == '/')
}

type Reported interface {
	Fact(key string) (any, bool)
}

func (action Action) Satisfied(reported Reported) (bool, bool) {
	var key, desired string
	switch action.Kind {
	case KindEnsureVersion:
		key, desired = action.VersionKey, action.Version
	case KindEnsureConfig:
		key, desired = ConfigHashKey, action.ConfigHash
	default:
		return false, false
	}
	value, found := reported.Fact(key)
	if !found {
		return false, false
	}
	text, isText := value.(string)
	if !isText {
		return false, true
	}
	return text == desired, true
}
