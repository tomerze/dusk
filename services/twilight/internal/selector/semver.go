package selector

import (
	"regexp"
	"strings"
)

const SemverPattern = `^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$`

var semverExpression = regexp.MustCompile(SemverPattern)

type versionKey [4]string

func semverKey(version string) (versionKey, bool) {
	match := semverExpression.FindStringSubmatch(version)
	if match == nil {
		return versionKey{}, false
	}
	release := "1"
	if match[4] != "" {
		release = "0"
	}
	return versionKey{match[1], match[2], match[3], release}, true
}

func compareDigits(left, right string) int {
	if len(left) != len(right) {
		if len(left) < len(right) {
			return -1
		}
		return 1
	}
	return strings.Compare(left, right)
}

func compareVersionKeys(left, right versionKey) int {
	for index := range left {
		if order := compareDigits(left[index], right[index]); order != 0 {
			return order
		}
	}
	return 0
}

func CompareVersions(left, right string) (int, bool) {
	leftKey, leftValid := semverKey(left)
	rightKey, rightValid := semverKey(right)
	if !leftValid || !rightValid {
		return 0, false
	}
	return compareVersionKeys(leftKey, rightKey), true
}
