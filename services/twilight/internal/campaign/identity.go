package campaign

import (
	"bytes"
	"crypto/rand"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"fmt"
	"strconv"

	"github.com/google/uuid"
)

type Pid uint64

const (
	DefaultShellPid Pid = 0xf2efce60e8c425d0
	reservedBelow   Pid = 1 << 16
	pidLabel            = "dusk-pid-v1"
)

var ErrMalformedPid = errors.New("a pid is a u64 in decimal without leading zeros, and not a reserved pid")

func Reserved(pid Pid) bool {
	return pid < reservedBelow || pid == DefaultShellPid
}

func (pid Pid) String() string {
	return strconv.FormatUint(uint64(pid), 10)
}

func ParsePid(text string) (Pid, error) {
	parsed, failure := strconv.ParseUint(text, 10, 64)
	if failure != nil || strconv.FormatUint(parsed, 10) != text {
		return 0, fmt.Errorf("%w: %q", ErrMalformedPid, text)
	}
	return Pid(parsed), nil
}

func (pid Pid) MarshalJSON() ([]byte, error) {
	if pid == 0 {
		return []byte("null"), nil
	}
	return []byte(`"` + pid.String() + `"`), nil
}

func (pid *Pid) UnmarshalJSON(document []byte) error {
	if bytes.Equal(document, []byte("null")) {
		*pid = 0
		return nil
	}
	if len(document) < 2 || document[0] != '"' || document[len(document)-1] != '"' {
		return fmt.Errorf("%w: %s", ErrMalformedPid, document)
	}
	parsed, failure := ParsePid(string(document[1 : len(document)-1]))
	if failure != nil {
		return failure
	}
	*pid = parsed
	return nil
}

func firstEight(input []byte) Pid {
	digest := sha256.Sum256(input)
	return Pid(binary.BigEndian.Uint64(digest[:8]))
}

func derive(input []byte, reserved func(Pid) bool) Pid {
	pid := firstEight(input)
	for counter := 1; reserved(pid); counter++ {
		pid = firstEight(append(append([]byte(nil), input...), "/"+strconv.Itoa(counter)...))
	}
	return pid
}

func pidInput(campaign uuid.UUID, device, installation string, attempt int) []byte {
	return []byte(pidLabel + campaign.String() + "/" + device + "/" + installation + "/" + strconv.Itoa(attempt))
}

func DerivePid(campaign uuid.UUID, device, installation string, attempt int) Pid {
	return derive(pidInput(campaign, device, installation, attempt), Reserved)
}

func RandomPid() (Pid, error) {
	drawn := make([]byte, 8)
	for {
		if _, failure := rand.Read(drawn); failure != nil {
			return 0, fmt.Errorf("draw a pid: %w", failure)
		}
		if pid := Pid(binary.BigEndian.Uint64(drawn)); !Reserved(pid) {
			return pid, nil
		}
	}
}

func Bucket(device, installation string, salt []byte) uint64 {
	hash := sha256.New()
	hash.Write([]byte(device))
	hash.Write([]byte{'/'})
	hash.Write([]byte(installation))
	hash.Write([]byte{'/'})
	hash.Write(salt)
	return binary.BigEndian.Uint64(hash.Sum(nil)[:8])
}
