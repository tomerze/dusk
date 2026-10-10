package selector

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
	"unicode/utf8"
)

type tokenKind int

const (
	tokenEnd tokenKind = iota
	tokenIdentifier
	tokenString
	tokenNumber
	tokenLeftParenthesis
	tokenRightParenthesis
	tokenLeftBracket
	tokenRightBracket
	tokenComma
	tokenEqual
	tokenNotEqual
	tokenLess
	tokenLessOrEqual
	tokenGreater
	tokenGreaterOrEqual
)

type token struct {
	kind     tokenKind
	text     string
	value    string
	position int
	end      int
}

func (kind tokenKind) describe() string {
	switch kind {
	case tokenEnd:
		return "the end of the selector"
	case tokenIdentifier:
		return "a name"
	case tokenString:
		return "a string"
	case tokenNumber:
		return "a number"
	case tokenLeftParenthesis:
		return "\"(\""
	case tokenRightParenthesis:
		return "\")\""
	case tokenLeftBracket:
		return "\"[\""
	case tokenRightBracket:
		return "\"]\""
	case tokenComma:
		return "\",\""
	case tokenEqual:
		return "\"==\""
	case tokenNotEqual:
		return "\"!=\""
	case tokenLess:
		return "\"<\""
	case tokenLessOrEqual:
		return "\"<=\""
	case tokenGreater:
		return "\">\""
	case tokenGreaterOrEqual:
		return "\">=\""
	}
	return "an unknown token"
}

const (
	maximumNumberLength = 100
	maximumExponent     = 1000
)

type lexer struct {
	source string
	offset int
	runeAt int
	tokens []token
}

var singleCharacterTokens = map[byte]tokenKind{
	'(': tokenLeftParenthesis,
	')': tokenRightParenthesis,
	'[': tokenLeftBracket,
	']': tokenRightBracket,
	',': tokenComma,
}

var operatorTokens = []struct {
	text string
	kind tokenKind
}{
	{"==", tokenEqual},
	{"!=", tokenNotEqual},
	{"<=", tokenLessOrEqual},
	{">=", tokenGreaterOrEqual},
	{"<", tokenLess},
	{">", tokenGreater},
}

func tokenize(source string) ([]token, error) {
	if !utf8.ValidString(source) {
		return nil, &Error{Position: 0, End: 0, Message: "the selector is not valid UTF-8"}
	}
	scanner := &lexer{source: source}
	for {
		next, failure := scanner.next()
		if failure != nil {
			return nil, failure
		}
		scanner.tokens = append(scanner.tokens, next)
		if next.kind == tokenEnd {
			return scanner.tokens, nil
		}
	}
}

func (scanner *lexer) advance(bytes int) {
	scanner.runeAt += utf8.RuneCountInString(scanner.source[scanner.offset : scanner.offset+bytes])
	scanner.offset += bytes
}

func (scanner *lexer) next() (token, error) {
	for scanner.offset < len(scanner.source) {
		character := scanner.source[scanner.offset]
		if character != ' ' && character != '\t' && character != '\n' && character != '\r' {
			break
		}
		scanner.advance(1)
	}
	start := scanner.runeAt
	if scanner.offset >= len(scanner.source) {
		return token{kind: tokenEnd, position: start, end: start}, nil
	}
	rest := scanner.source[scanner.offset:]
	character := rest[0]
	if kind, found := singleCharacterTokens[character]; found {
		scanner.advance(1)
		return token{kind: kind, text: rest[:1], position: start, end: start + 1}, nil
	}
	for _, operator := range operatorTokens {
		if strings.HasPrefix(rest, operator.text) {
			scanner.advance(len(operator.text))
			return token{kind: operator.kind, text: operator.text, position: start, end: start + len(operator.text)}, nil
		}
	}
	switch {
	case character == '"':
		return scanner.stringLiteral(start)
	case character == '-' || (character >= '0' && character <= '9'):
		return scanner.numberLiteral(start)
	case isIdentifierStart(character):
		length := 1
		for length < len(rest) && isIdentifierPart(rest[length]) {
			length++
		}
		scanner.advance(length)
		return token{kind: tokenIdentifier, text: rest[:length], position: start, end: start + length}, nil
	case character == '=':
		return token{}, &Error{Position: start, End: start + 1, Message: "use \"==\" to compare"}
	case character == '!':
		return token{}, &Error{Position: start, End: start + 1, Message: "use \"not\" to negate, or \"!=\" to compare"}
	case character == '\'':
		return token{}, &Error{Position: start, End: start + 1, Message: "strings use double quotes"}
	}
	symbol, _ := utf8.DecodeRuneInString(rest)
	return token{}, &Error{Position: start, End: start + 1, Message: fmt.Sprintf("unexpected character %q", symbol)}
}

func isIdentifierStart(character byte) bool {
	return character == '_' || (character >= 'a' && character <= 'z') || (character >= 'A' && character <= 'Z')
}

func isIdentifierPart(character byte) bool {
	return isIdentifierStart(character) || (character >= '0' && character <= '9')
}

func (scanner *lexer) stringLiteral(start int) (token, error) {
	rest := scanner.source[scanner.offset:]
	length := 1
	for {
		if length >= len(rest) {
			end := start + utf8.RuneCountInString(rest)
			return token{}, &Error{Position: start, End: end, Message: "this string is not closed"}
		}
		character := rest[length]
		if character == '\\' {
			length += 2
			continue
		}
		length++
		if character == '"' {
			break
		}
	}
	text := rest[:length]
	end := start + utf8.RuneCountInString(text)
	var value string
	if failure := json.Unmarshal([]byte(text), &value); failure != nil {
		return token{}, &Error{Position: start, End: end, Message: "this string has an invalid escape or control character"}
	}
	if strings.ContainsRune(value, 0) {
		return token{}, &Error{Position: start, End: end, Message: "strings cannot contain U+0000"}
	}
	scanner.advance(length)
	return token{kind: tokenString, text: text, value: value, position: start, end: end}, nil
}

func (scanner *lexer) numberLiteral(start int) (token, error) {
	rest := scanner.source[scanner.offset:]
	length := 0
	digits := func() int {
		count := 0
		for length < len(rest) && rest[length] >= '0' && rest[length] <= '9' {
			length++
			count++
		}
		return count
	}
	invalid := func() (token, error) {
		for length < len(rest) && (isIdentifierPart(rest[length]) || rest[length] == '.' || rest[length] == '-' || rest[length] == '+') {
			length++
		}
		return token{}, &Error{Position: start, End: start + length, Message: fmt.Sprintf("%q is not a number", rest[:length])}
	}
	if rest[length] == '-' {
		length++
	}
	integerStart := length
	if digits() == 0 {
		return invalid()
	}
	if rest[integerStart] == '0' && length-integerStart > 1 {
		return invalid()
	}
	if length < len(rest) && rest[length] == '.' {
		length++
		if digits() == 0 {
			return invalid()
		}
	}
	exponentStart := -1
	if length < len(rest) && (rest[length] == 'e' || rest[length] == 'E') {
		length++
		exponentStart = length
		if length < len(rest) && (rest[length] == '+' || rest[length] == '-') {
			length++
		}
		if digits() == 0 {
			return invalid()
		}
	}
	if length < len(rest) && (isIdentifierPart(rest[length]) || rest[length] == '.') {
		return invalid()
	}
	text := rest[:length]
	if length > maximumNumberLength {
		return token{}, &Error{Position: start, End: start + length, Message: fmt.Sprintf("a number has at most %d characters", maximumNumberLength)}
	}
	if exponentStart >= 0 {
		exponent, failure := strconv.Atoi(rest[exponentStart:length])
		if failure != nil || exponent < -maximumExponent || exponent > maximumExponent {
			return token{}, &Error{Position: start, End: start + length, Message: fmt.Sprintf("a number's exponent is between -%d and %d", maximumExponent, maximumExponent)}
		}
	}
	scanner.advance(length)
	return token{kind: tokenNumber, text: text, value: text, position: start, end: start + length}, nil
}
