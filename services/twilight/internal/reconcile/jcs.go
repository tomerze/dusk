package reconcile

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"sort"
	"strconv"
	"strings"
	"unicode/utf16"
	"unicode/utf8"
)

func decodeDocument(document []byte) (any, error) {
	if !utf8.Valid(document) {
		return nil, errors.New("the document is not valid UTF-8")
	}
	decoder := json.NewDecoder(bytes.NewReader(document))
	decoder.UseNumber()
	var value any
	if failure := decoder.Decode(&value); failure != nil {
		return nil, failure
	}
	if _, failure := decoder.Token(); !errors.Is(failure, io.EOF) {
		return nil, errors.New("trailing data after the JSON value")
	}
	return value, nil
}

func canonicalBytes(value any) ([]byte, error) {
	var output bytes.Buffer
	if failure := canonicalValue(&output, value); failure != nil {
		return nil, failure
	}
	return output.Bytes(), nil
}

func Canonicalize(document []byte) ([]byte, error) {
	value, failure := decodeDocument(document)
	if failure != nil {
		return nil, failure
	}
	return canonicalBytes(value)
}

func EntryHash(entry []byte) (string, error) {
	value, failure := decodeDocument(entry)
	if failure != nil {
		return "", failure
	}
	members, isObject := value.(map[string]any)
	if !isObject {
		return "", errors.New("a ledger entry is a JSON object")
	}
	delete(members, "hash")
	canonical, failure := canonicalBytes(members)
	if failure != nil {
		return "", failure
	}
	digest := sha256.Sum256(canonical)
	return hex.EncodeToString(digest[:]), nil
}

func CanonicalizeValue(value any) ([]byte, error) {
	encoded, failure := json.Marshal(value)
	if failure != nil {
		return nil, failure
	}
	return Canonicalize(encoded)
}

func canonicalValue(output *bytes.Buffer, value any) error {
	switch typed := value.(type) {
	case nil:
		output.WriteString("null")
	case bool:
		if typed {
			output.WriteString("true")
		} else {
			output.WriteString("false")
		}
	case json.Number:
		formatted, failure := FormatNumber(string(typed))
		if failure != nil {
			return failure
		}
		output.WriteString(formatted)
	case string:
		return canonicalString(output, typed)
	case []any:
		output.WriteByte('[')
		for index, element := range typed {
			if index > 0 {
				output.WriteByte(',')
			}
			if failure := canonicalValue(output, element); failure != nil {
				return failure
			}
		}
		output.WriteByte(']')
	case map[string]any:
		keys := make([]string, 0, len(typed))
		for key := range typed {
			keys = append(keys, key)
		}
		sort.Slice(keys, func(left, right int) bool { return lessUTF16(keys[left], keys[right]) })
		output.WriteByte('{')
		for index, key := range keys {
			if index > 0 {
				output.WriteByte(',')
			}
			if failure := canonicalString(output, key); failure != nil {
				return failure
			}
			output.WriteByte(':')
			if failure := canonicalValue(output, typed[key]); failure != nil {
				return failure
			}
		}
		output.WriteByte('}')
	default:
		return fmt.Errorf("unsupported JSON value %T", value)
	}
	return nil
}

func lessUTF16(left, right string) bool {
	leftUnits := utf16.Encode([]rune(left))
	rightUnits := utf16.Encode([]rune(right))
	for index := 0; index < len(leftUnits) && index < len(rightUnits); index++ {
		if leftUnits[index] != rightUnits[index] {
			return leftUnits[index] < rightUnits[index]
		}
	}
	return len(leftUnits) < len(rightUnits)
}

const hexDigits = "0123456789abcdef"

func canonicalString(output *bytes.Buffer, text string) error {
	if !utf8.ValidString(text) {
		return errors.New("a string is not valid UTF-8")
	}
	output.WriteByte('"')
	for _, character := range text {
		switch character {
		case '"':
			output.WriteString(`\"`)
		case '\\':
			output.WriteString(`\\`)
		case '\b':
			output.WriteString(`\b`)
		case '\f':
			output.WriteString(`\f`)
		case '\n':
			output.WriteString(`\n`)
		case '\r':
			output.WriteString(`\r`)
		case '\t':
			output.WriteString(`\t`)
		default:
			if character < 0x20 {
				output.WriteString(`\u00`)
				output.WriteByte(hexDigits[character>>4])
				output.WriteByte(hexDigits[character&0xf])
			} else {
				output.WriteRune(character)
			}
		}
	}
	output.WriteByte('"')
	return nil
}

func FormatNumber(text string) (string, error) {
	value, failure := strconv.ParseFloat(text, 64)
	if failure != nil || math.IsInf(value, 0) || math.IsNaN(value) {
		return "", fmt.Errorf("%q is not a finite IEEE 754 double", text)
	}
	if value == 0 {
		return "0", nil
	}
	sign := ""
	if value < 0 {
		sign = "-"
		value = -value
	}
	scientific := strconv.FormatFloat(value, 'e', -1, 64)
	mantissa, exponentText, _ := strings.Cut(scientific, "e")
	digits := strings.Replace(mantissa, ".", "", 1)
	exponent, _ := strconv.Atoi(exponentText)
	length := len(digits)
	point := exponent + 1
	switch {
	case length <= point && point <= 21:
		return sign + digits + strings.Repeat("0", point-length), nil
	case 0 < point && point <= 21:
		return sign + digits[:point] + "." + digits[point:], nil
	case -6 < point && point <= 0:
		return sign + "0." + strings.Repeat("0", -point) + digits, nil
	}
	exponentSign := "+"
	if point-1 < 0 {
		exponentSign = "-"
	}
	magnitude := strconv.Itoa(int(math.Abs(float64(point - 1))))
	if length == 1 {
		return sign + digits + "e" + exponentSign + magnitude, nil
	}
	return sign + digits[:1] + "." + digits[1:] + "e" + exponentSign + magnitude, nil
}
