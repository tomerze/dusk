package selector

import (
	"fmt"
	"math/big"
	"unicode/utf8"
)

const (
	MaximumLength      = 16384
	maximumDepth       = 64
	maximumListLength  = 1024
	maximumComparisons = 1024
)

var keywords = map[string]bool{
	"and": true, "or": true, "not": true, "in": true, "has": true,
	"semver": true, "facts": true, "true": true, "false": true,
}

var comparisonOperators = map[tokenKind]Operator{
	tokenEqual:          OperatorEqual,
	tokenNotEqual:       OperatorNotEqual,
	tokenLess:           OperatorLess,
	tokenLessOrEqual:    OperatorLessOrEqual,
	tokenGreater:        OperatorGreater,
	tokenGreaterOrEqual: OperatorGreaterOrEqual,
}

type Selector struct {
	Source string
	Root   Expression
}

type parser struct {
	tokens      []token
	index       int
	depth       int
	comparisons int
}

func Parse(source string) (*Selector, error) {
	if utf8.RuneCountInString(source) > MaximumLength {
		return nil, &Error{Position: MaximumLength, End: MaximumLength, Message: fmt.Sprintf("the selector is longer than %d characters", MaximumLength)}
	}
	tokens, failure := tokenize(source)
	if failure != nil {
		return nil, failure
	}
	if len(tokens) == 1 {
		return &Selector{Source: source, Root: &MatchAll{}}, nil
	}
	state := &parser{tokens: tokens}
	root, failure := state.or()
	if failure != nil {
		return nil, failure
	}
	if state.peek().kind != tokenEnd {
		return nil, state.unexpected("\"and\", \"or\" or the end of the selector")
	}
	return &Selector{Source: source, Root: root}, nil
}

func (selector *Selector) MatchesAll() bool {
	_, all := selector.Root.(*MatchAll)
	return all
}

func (state *parser) peek() token {
	return state.tokens[state.index]
}

func (state *parser) take() token {
	current := state.tokens[state.index]
	if current.kind != tokenEnd {
		state.index++
	}
	return current
}

func (state *parser) keyword(word string) bool {
	current := state.peek()
	return current.kind == tokenIdentifier && current.text == word
}

func (state *parser) unexpected(expected string) error {
	current := state.peek()
	found := current.kind.describe()
	if current.kind == tokenIdentifier || current.kind == tokenNumber || current.kind == tokenString {
		found = fmt.Sprintf("%q", current.text)
	}
	return &Error{Position: current.position, End: max(current.end, current.position+1), Message: fmt.Sprintf("expected %s, found %s", expected, found)}
}

func (state *parser) expect(kind tokenKind) (token, error) {
	if state.peek().kind != kind {
		return token{}, state.unexpected(kind.describe())
	}
	return state.take(), nil
}

func (state *parser) enter() error {
	state.depth++
	if state.depth > maximumDepth {
		current := state.peek()
		return &Error{Position: current.position, End: current.end, Message: fmt.Sprintf("the selector nests deeper than %d levels", maximumDepth)}
	}
	return nil
}

func (state *parser) or() (Expression, error) {
	left, failure := state.and()
	if failure != nil {
		return nil, failure
	}
	for state.keyword("or") {
		state.take()
		right, failure := state.and()
		if failure != nil {
			return nil, failure
		}
		left = &Or{Left: left, Right: right}
	}
	return left, nil
}

func (state *parser) and() (Expression, error) {
	left, failure := state.unary()
	if failure != nil {
		return nil, failure
	}
	for state.keyword("and") {
		state.take()
		right, failure := state.unary()
		if failure != nil {
			return nil, failure
		}
		left = &And{Left: left, Right: right}
	}
	return left, nil
}

func (state *parser) unary() (Expression, error) {
	if failure := state.enter(); failure != nil {
		return nil, failure
	}
	defer func() { state.depth-- }()
	if state.keyword("not") {
		position := state.take().position
		operand, failure := state.unary()
		if failure != nil {
			return nil, failure
		}
		return &Not{Operand: operand, Position: position}, nil
	}
	return state.primary()
}

func (state *parser) primary() (Expression, error) {
	current := state.peek()
	if current.kind == tokenLeftParenthesis {
		state.take()
		inner, failure := state.or()
		if failure != nil {
			return nil, failure
		}
		if _, failure := state.expect(tokenRightParenthesis); failure != nil {
			return nil, failure
		}
		return inner, nil
	}
	if state.keyword("has") {
		state.take()
		if _, failure := state.expect(tokenLeftParenthesis); failure != nil {
			return nil, failure
		}
		field, failure := state.field()
		if failure != nil {
			return nil, failure
		}
		closing, failure := state.expect(tokenRightParenthesis)
		if failure != nil {
			return nil, failure
		}
		return &Has{Field: field, Position: current.position, End: closing.end}, nil
	}
	return state.comparison()
}

func (state *parser) field() (Field, error) {
	current := state.peek()
	if current.kind != tokenIdentifier {
		return Field{}, state.unexpected("a field name or facts[\"key\"]")
	}
	if current.text == "facts" {
		state.take()
		if _, failure := state.expect(tokenLeftBracket); failure != nil {
			return Field{}, failure
		}
		key := state.peek()
		if key.kind != tokenString {
			return Field{}, state.unexpected("the fact name as a string")
		}
		state.take()
		if key.value == "" {
			return Field{}, &Error{Position: key.position, End: key.end, Message: "a fact name cannot be empty"}
		}
		closing, failure := state.expect(tokenRightBracket)
		if failure != nil {
			return Field{}, failure
		}
		return Field{Kind: FieldFact, FactKey: key.value, Position: current.position, End: closing.end}, nil
	}
	kind, known := columns[current.text]
	if !known {
		message := fmt.Sprintf("unknown field %q; use facts[%q] for a fact", current.text, current.text)
		if keywords[current.text] {
			message = fmt.Sprintf("expected a field name, found %q", current.text)
		}
		return Field{}, &Error{Position: current.position, End: current.end, Message: message}
	}
	state.take()
	return Field{Kind: kind, Column: current.text, Position: current.position, End: current.end}, nil
}

func (state *parser) comparison() (Expression, error) {
	start := state.peek().position
	semver := false
	var field Field
	var failure error
	if state.keyword("semver") {
		state.take()
		semver = true
		if _, failure = state.expect(tokenLeftParenthesis); failure != nil {
			return nil, failure
		}
		if field, failure = state.field(); failure != nil {
			return nil, failure
		}
		if _, failure = state.expect(tokenRightParenthesis); failure != nil {
			return nil, failure
		}
	} else if field, failure = state.field(); failure != nil {
		return nil, failure
	}
	operatorToken := state.peek()
	var operator Operator
	if found, known := comparisonOperators[operatorToken.kind]; known {
		operator = found
		state.take()
	} else if state.keyword("in") {
		operator = OperatorIn
		state.take()
	} else if state.keyword("not") && state.tokens[state.index+1].kind == tokenIdentifier && state.tokens[state.index+1].text == "in" {
		operator = OperatorNotIn
		state.take()
		state.take()
	} else {
		return nil, state.unexpected("a comparison (==, !=, <, <=, >, >=, in, not in)")
	}
	state.comparisons++
	if state.comparisons > maximumComparisons {
		return nil, &Error{Position: start, End: operatorToken.end, Message: fmt.Sprintf("the selector has more than %d comparisons", maximumComparisons)}
	}
	var values []Literal
	end := 0
	if operator == OperatorIn || operator == OperatorNotIn {
		opening, failure := state.expect(tokenLeftBracket)
		if failure != nil {
			return nil, failure
		}
		for {
			value, failure := state.literal()
			if failure != nil {
				return nil, failure
			}
			values = append(values, value)
			if len(values) > maximumListLength {
				return nil, &Error{Position: opening.position, End: value.End, Message: fmt.Sprintf("a list holds at most %d values", maximumListLength)}
			}
			if state.peek().kind == tokenComma {
				state.take()
				continue
			}
			closing, failure := state.expect(tokenRightBracket)
			if failure != nil {
				return nil, failure
			}
			end = closing.end
			break
		}
	} else {
		value, failure := state.literal()
		if failure != nil {
			return nil, failure
		}
		values = []Literal{value}
		end = value.End
	}
	comparison := &Comparison{Field: field, Semver: semver, Operator: operator, Values: values, Position: start, End: end}
	if failure := check(comparison); failure != nil {
		return nil, failure
	}
	return comparison, nil
}

func (state *parser) literal() (Literal, error) {
	current := state.peek()
	switch {
	case current.kind == tokenString:
		state.take()
		return Literal{Kind: LiteralString, Text: current.value, Position: current.position, End: current.end}, nil
	case current.kind == tokenNumber:
		state.take()
		number, _ := new(big.Rat).SetString(current.value)
		return Literal{Kind: LiteralNumber, Text: current.value, Number: number, Position: current.position, End: current.end}, nil
	case state.keyword("true"), state.keyword("false"):
		state.take()
		return Literal{Kind: LiteralBool, Text: current.text, Bool: current.text == "true", Position: current.position, End: current.end}, nil
	}
	return Literal{}, state.unexpected("a string, number, true or false")
}

func check(comparison *Comparison) error {
	field := comparison.Field
	first := comparison.Values[0]
	for _, value := range comparison.Values[1:] {
		if value.Kind != first.Kind {
			return &Error{Position: value.Position, End: value.End, Message: fmt.Sprintf("a list holds values of one type; this list starts with %s", first.Kind)}
		}
	}
	if comparison.Semver || (field.Kind == FieldVersion && comparison.Operator.ordering()) {
		for _, value := range comparison.Values {
			if value.Kind != LiteralString {
				return &Error{Position: value.Position, End: value.End, Message: fmt.Sprintf("%s compares with version strings like \"1.2.3\"", describeOperand(comparison))}
			}
			if _, valid := semverKey(value.Text); !valid {
				return &Error{Position: value.Position, End: value.End, Message: fmt.Sprintf("%q is not a semantic version like \"1.2.3\"", value.Text)}
			}
		}
		return nil
	}
	if field.Kind != FieldFact {
		if first.Kind != LiteralString {
			return &Error{Position: first.Position, End: first.End, Message: fmt.Sprintf("%s compares with strings, not %s", field.Column, first.Kind)}
		}
		return nil
	}
	if first.Kind == LiteralBool && comparison.Operator.ordering() {
		return &Error{Position: comparison.Position, End: comparison.End, Message: "booleans can only be compared with ==, !=, in and not in"}
	}
	return nil
}

func describeOperand(comparison *Comparison) string {
	if comparison.Semver {
		return fmt.Sprintf("semver(%s)", comparison.Field.name())
	}
	return comparison.Field.name()
}
