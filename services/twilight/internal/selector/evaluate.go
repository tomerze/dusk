package selector

import (
	"encoding/json"
	"math/big"
	"strings"
)

type Record interface {
	Column(name string) (string, bool)
	Fact(key string) (any, bool)
}

func (selector *Selector) Matches(record Record) bool {
	return evaluate(selector.Root, record)
}

func evaluate(expression Expression, record Record) bool {
	switch node := expression.(type) {
	case *MatchAll:
		return true
	case *And:
		return evaluate(node.Left, record) && evaluate(node.Right, record)
	case *Or:
		return evaluate(node.Left, record) || evaluate(node.Right, record)
	case *Not:
		return !evaluate(node.Operand, record)
	case *Has:
		if node.Field.Kind == FieldFact {
			_, present := record.Fact(node.Field.FactKey)
			return present
		}
		_, present := record.Column(node.Field.Column)
		return present
	case *Comparison:
		return compare(node, record)
	}
	return false
}

func holds(operator Operator, order int) bool {
	switch operator {
	case OperatorEqual:
		return order == 0
	case OperatorNotEqual:
		return order != 0
	case OperatorLess:
		return order < 0
	case OperatorLessOrEqual:
		return order <= 0
	case OperatorGreater:
		return order > 0
	case OperatorGreaterOrEqual:
		return order >= 0
	}
	return false
}

func membership(operator Operator, values []Literal, equal func(Literal) bool) bool {
	for _, value := range values {
		if equal(value) {
			return operator == OperatorIn
		}
	}
	return operator == OperatorNotIn
}

func compare(comparison *Comparison, record Record) bool {
	field := comparison.Field
	var text string
	var present bool
	if field.Kind == FieldFact {
		value, found := record.Fact(field.FactKey)
		if !found {
			return false
		}
		if !comparison.Semver {
			return compareFact(comparison, value)
		}
		text, present = value.(string)
	} else {
		text, present = record.Column(field.Column)
	}
	if !present {
		return false
	}
	if comparison.Semver || (field.Kind == FieldVersion && comparison.Operator.ordering()) {
		key, valid := semverKey(text)
		if !valid {
			return false
		}
		order := func(value Literal) int {
			literalKey, _ := semverKey(value.Text)
			return compareVersionKeys(key, literalKey)
		}
		if comparison.Operator == OperatorIn || comparison.Operator == OperatorNotIn {
			return membership(comparison.Operator, comparison.Values, func(value Literal) bool { return order(value) == 0 })
		}
		return holds(comparison.Operator, order(comparison.Values[0]))
	}
	if comparison.Operator == OperatorIn || comparison.Operator == OperatorNotIn {
		return membership(comparison.Operator, comparison.Values, func(value Literal) bool { return value.Text == text })
	}
	return holds(comparison.Operator, strings.Compare(text, comparison.Values[0].Text))
}

func number(value any) (*big.Rat, bool) {
	switch typed := value.(type) {
	case json.Number:
		return new(big.Rat).SetString(string(typed))
	case float64:
		rational := new(big.Rat)
		if rational.SetFloat64(typed) == nil {
			return nil, false
		}
		return rational, true
	case int64:
		return new(big.Rat).SetInt64(typed), true
	case int:
		return new(big.Rat).SetInt64(int64(typed)), true
	}
	return nil, false
}

func compareFact(comparison *Comparison, value any) bool {
	kind := comparison.Values[0].Kind
	var order func(Literal) int
	switch kind {
	case LiteralString:
		text, typed := value.(string)
		if !typed {
			return false
		}
		order = func(literal Literal) int { return strings.Compare(text, literal.Text) }
	case LiteralNumber:
		actual, typed := number(value)
		if !typed {
			return false
		}
		order = func(literal Literal) int { return actual.Cmp(literal.Number) }
	case LiteralBool:
		actual, typed := value.(bool)
		if !typed {
			return false
		}
		order = func(literal Literal) int {
			if actual == literal.Bool {
				return 0
			}
			return 1
		}
	}
	if comparison.Operator == OperatorIn || comparison.Operator == OperatorNotIn {
		return membership(comparison.Operator, comparison.Values, func(literal Literal) bool { return order(literal) == 0 })
	}
	return holds(comparison.Operator, order(comparison.Values[0]))
}
