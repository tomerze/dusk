package selector

import (
	"encoding/json"
	"fmt"
	"strings"
)

type Compiled struct {
	SQL        string
	Parameters []string
}

type compiler struct {
	alias      string
	first      int
	parameters []string
}

func (selector *Selector) Compile(alias string, firstParameter int) Compiled {
	state := &compiler{alias: alias, first: firstParameter}
	sql := state.expression(selector.Root)
	return Compiled{SQL: sql, Parameters: state.parameters}
}

func (state *compiler) parameter(value string) string {
	state.parameters = append(state.parameters, value)
	return fmt.Sprintf("$%d", state.first+len(state.parameters)-1)
}

func (state *compiler) column(name string) string {
	if state.alias == "" {
		return name
	}
	return state.alias + "." + name
}

func (state *compiler) expression(expression Expression) string {
	switch node := expression.(type) {
	case *MatchAll:
		return "true"
	case *And:
		return "(" + state.expression(node.Left) + " and " + state.expression(node.Right) + ")"
	case *Or:
		return "(" + state.expression(node.Left) + " or " + state.expression(node.Right) + ")"
	case *Not:
		return "((" + state.expression(node.Operand) + ") is not true)"
	case *Has:
		if node.Field.Kind == FieldFact {
			return "(" + state.column("facts") + " ? " + state.parameter(node.Field.FactKey) + "::text)"
		}
		return "(" + state.column(node.Field.Column) + " is not null)"
	case *Comparison:
		return state.comparison(node)
	}
	panic(fmt.Sprintf("selector: unknown expression %T", expression))
}

func sqlOperator(operator Operator) string {
	switch operator {
	case OperatorEqual:
		return "="
	case OperatorNotEqual:
		return "<>"
	case OperatorLess:
		return "<"
	case OperatorLessOrEqual:
		return "<="
	case OperatorGreater:
		return ">"
	case OperatorGreaterOrEqual:
		return ">="
	}
	panic(fmt.Sprintf("selector: no SQL operator for %s", operator))
}

func castFor(kind LiteralKind) string {
	switch kind {
	case LiteralNumber:
		return "numeric"
	case LiteralBool:
		return "boolean"
	}
	return "text"
}

func jsonTypeFor(kind LiteralKind) string {
	switch kind {
	case LiteralNumber:
		return "number"
	case LiteralBool:
		return "boolean"
	}
	return "string"
}

func literalJSON(literal Literal) string {
	if literal.Kind != LiteralString {
		return literal.Text
	}
	encoded, _ := json.Marshal(literal.Text)
	return string(encoded)
}

func (state *compiler) list(values []Literal, separator string, render func(Literal) string) string {
	parts := make([]string, len(values))
	for index, value := range values {
		parts[index] = render(value)
	}
	return strings.Join(parts, separator)
}

func (state *compiler) comparison(comparison *Comparison) string {
	field := comparison.Field
	operator := comparison.Operator
	if field.Kind != FieldFact {
		column := state.column(field.Column)
		if comparison.Semver || (field.Kind == FieldVersion && operator.ordering()) {
			return state.semverComparison("semver_key("+column+")", comparison)
		}
		text := func(value Literal) string { return state.parameter(value.Text) + "::text" }
		switch operator {
		case OperatorIn:
			return "(" + column + " in (" + state.list(comparison.Values, ", ", text) + "))"
		case OperatorNotIn:
			return "(" + column + " not in (" + state.list(comparison.Values, ", ", text) + "))"
		case OperatorEqual, OperatorNotEqual:
			return "(" + column + " " + sqlOperator(operator) + " " + text(comparison.Values[0]) + ")"
		}
		return "(" + column + " collate \"C\" " + sqlOperator(operator) + " " + text(comparison.Values[0]) + ")"
	}
	facts := state.column("facts")
	key := func() string { return state.parameter(field.FactKey) + "::text" }
	typed := func(factKey, jsonType, body string) string {
		return "(case when jsonb_typeof(" + facts + " -> " + factKey + ") = '" + jsonType + "' then " + body + " end)"
	}
	if comparison.Semver {
		factKey := key()
		return typed(factKey, "string", state.semverComparison("semver_key("+facts+" ->> "+factKey+")", comparison))
	}
	kind := comparison.Values[0].Kind
	contains := func(value Literal) string {
		object, _ := json.Marshal(map[string]json.RawMessage{field.FactKey: json.RawMessage(literalJSON(value))})
		return "(" + facts + " @> " + state.parameter(string(object)) + "::jsonb)"
	}
	switch operator {
	case OperatorEqual:
		return contains(comparison.Values[0])
	case OperatorIn:
		return "(" + state.list(comparison.Values, " or ", contains) + ")"
	}
	factKey := key()
	extracted := "(" + facts + " ->> " + factKey + ")"
	if kind != LiteralString {
		extracted += "::" + castFor(kind)
	}
	cast := func(value Literal) string { return state.parameter(value.Text) + "::" + castFor(kind) }
	switch operator {
	case OperatorNotEqual:
		return typed(factKey, jsonTypeFor(kind), extracted+" <> "+cast(comparison.Values[0]))
	case OperatorNotIn:
		return typed(factKey, jsonTypeFor(kind), extracted+" not in ("+state.list(comparison.Values, ", ", cast)+")")
	}
	if kind == LiteralString {
		return typed(factKey, "string", extracted+" collate \"C\" "+sqlOperator(operator)+" "+cast(comparison.Values[0]))
	}
	return typed(factKey, "number", extracted+" "+sqlOperator(operator)+" "+cast(comparison.Values[0]))
}

func (state *compiler) semverComparison(left string, comparison *Comparison) string {
	key := func(value Literal) string { return "semver_key(" + state.parameter(value.Text) + "::text)" }
	switch comparison.Operator {
	case OperatorIn:
		return "(" + left + " in (" + state.list(comparison.Values, ", ", key) + "))"
	case OperatorNotIn:
		return "(" + left + " not in (" + state.list(comparison.Values, ", ", key) + "))"
	}
	return "(" + left + " " + sqlOperator(comparison.Operator) + " " + key(comparison.Values[0]) + ")"
}
