package inventory

import (
	"bufio"
	_ "embed"
	"encoding/json"
	"math"
	"math/bits"
	"strconv"
	"strings"
)

//go:embed zones.tsv
var zoneTable string

var zoneCountries = func() map[string]string {
	countries := map[string]string{}
	scanner := bufio.NewScanner(strings.NewReader(zoneTable))
	for scanner.Scan() {
		zone, country, found := strings.Cut(scanner.Text(), "\t")
		if found {
			countries[zone] = country
		}
	}
	return countries
}()

func CountryOfZone(zone string) (string, bool) {
	country, found := zoneCountries[zone]
	return country, found
}

const DeviceIDFact = "dusk.device.id"

type Columns struct {
	Country       *string
	OSName        *string
	OSVersion     *string
	OSBuild       *string
	DuskVersion   *string
	HardwareClass *string
	Locale        *string
	Hostname      *string
	Impl          *string
	TargetArch    *string
}

type factReader map[string]json.RawMessage

func (facts factReader) text(key string) (string, bool) {
	raw, found := facts[key]
	if !found {
		return "", false
	}
	var text string
	if json.Unmarshal(raw, &text) == nil {
		text = strings.TrimSpace(text)
		return text, text != ""
	}
	var number json.Number
	if json.Unmarshal(raw, &number) == nil {
		return number.String(), true
	}
	return "", false
}

func (facts factReader) pointer(key string) *string {
	if text, found := facts.text(key); found {
		return &text
	}
	return nil
}

func (facts factReader) number(key string) (float64, bool) {
	raw, found := facts[key]
	if !found {
		return 0, false
	}
	var number float64
	if json.Unmarshal(raw, &number) == nil && !math.IsNaN(number) && number > 0 {
		return number, true
	}
	if text, found := facts.text(key); found {
		parsed, failure := strconv.ParseFloat(text, 64)
		return parsed, failure == nil && parsed > 0
	}
	return 0, false
}

func first(values ...*string) *string {
	for _, value := range values {
		if value != nil {
			return value
		}
	}
	return nil
}

func powerOfTwoAtLeast(value float64) int {
	if value <= 1 {
		return 1
	}
	return 1 << bits.Len(uint(math.Ceil(value)-1))
}

func powerOfTwoAtMost(value float64) int {
	if value < 2 {
		return 1
	}
	return 1 << (bits.Len(uint(value)) - 1)
}

func hardwareClass(facts factReader) *string {
	architecture, found := facts.text("dusk.target.arch")
	if !found {
		return nil
	}
	class := architecture
	if cores, found := facts.number("dusk.device.cores"); found {
		class += "-" + strconv.Itoa(powerOfTwoAtMost(cores)) + "c"
	}
	if memory, found := facts.number("dusk.device.memory_bytes"); found {
		class += "-" + strconv.Itoa(powerOfTwoAtLeast(memory/(1<<30)*0.9)) + "g"
	}
	return &class
}

func windowsBuild(facts factReader) *string {
	build, found := facts.text("dusk.os.windows.build_number")
	if !found {
		return nil
	}
	if revision, found := facts.text("dusk.os.windows.revision"); found {
		build += "." + revision
	}
	return &build
}

func Normalize(facts map[string]json.RawMessage) (Columns, map[string]json.RawMessage) {
	cleaned := make(map[string]json.RawMessage, len(facts))
	for key, value := range facts {
		if key != DeviceIDFact {
			cleaned[key] = value
		}
	}
	reader := factReader(cleaned)
	columns := Columns{
		DuskVersion:   reader.pointer("dusk.version"),
		HardwareClass: hardwareClass(reader),
		Locale:        reader.pointer("dusk.os.locale"),
		Hostname:      reader.pointer("dusk.hostname"),
		Impl:          reader.pointer("dusk.impl"),
		TargetArch:    reader.pointer("dusk.target.arch"),
	}
	operatingSystem, _ := reader.text("dusk.target.os")
	switch operatingSystem {
	case "linux":
		columns.OSName = reader.pointer("dusk.os.linux.os_release.id")
		columns.OSVersion = reader.pointer("dusk.os.linux.os_release.version_id")
		columns.OSBuild = reader.pointer("dusk.os.nix.uname.release")
	case "windows":
		columns.OSName = first(reader.pointer("dusk.os.windows.edition"), &operatingSystem)
		columns.OSVersion = first(reader.pointer("dusk.os.windows.display_version"), reader.pointer("dusk.os.windows.major_version"))
		columns.OSBuild = windowsBuild(reader)
	case "android":
		columns.OSName = first(reader.pointer("dusk.os.android.brand"), &operatingSystem)
		columns.OSVersion = reader.pointer("dusk.os.android.release")
		columns.OSBuild = reader.pointer("dusk.os.android.incremental")
	case "macos":
		columns.OSName = &operatingSystem
		columns.OSVersion = reader.pointer("dusk.os.macos.product_version")
		columns.OSBuild = reader.pointer("dusk.os.macos.build_version")
	case "ios":
		columns.OSName = &operatingSystem
		columns.OSVersion = reader.pointer("dusk.os.ios.product_version")
		columns.OSBuild = reader.pointer("dusk.os.ios.build_version")
	case "":
	default:
		columns.OSName = &operatingSystem
		columns.OSBuild = reader.pointer("dusk.os.nix.uname.release")
	}
	if zone, found := reader.text("dusk.os.time_zone"); found {
		if country, known := CountryOfZone(zone); known {
			columns.Country = &country
		}
	}
	return columns, cleaned
}
