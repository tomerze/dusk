package selector

import (
	"fmt"
	"math/big"
)

type Error struct {
	Position int    `json:"position"`
	End      int    `json:"end"`
	Message  string `json:"message"`
}

func (failure *Error) Error() string {
	return fmt.Sprintf("selector error at %d: %s", failure.Position, failure.Message)
}

type Operator int

const (
	OperatorEqual Operator = iota
	OperatorNotEqual
	OperatorLess
	OperatorLessOrEqual
	OperatorGreater
	OperatorGreaterOrEqual
	OperatorIn
	OperatorNotIn
)

func (operator Operator) String() string {
	return [...]string{"==", "!=", "<", "<=", ">", ">=", "in", "not in"}[operator]
}

func (operator Operator) ordering() bool {
	return operator >= OperatorLess && operator <= OperatorGreaterOrEqual
}

type LiteralKind int

const (
	LiteralString LiteralKind = iota
	LiteralNumber
	LiteralBool
)

func (kind LiteralKind) String() string {
	return [...]string{"strings", "numbers", "booleans"}[kind]
}

type Literal struct {
	Kind     LiteralKind
	Text     string
	Bool     bool
	Number   *big.Rat
	Position int
	End      int
}

type FieldKind int

const (
	FieldText FieldKind = iota
	FieldVersion
	FieldFact
)

type Field struct {
	Kind     FieldKind
	Column   string
	FactKey  string
	Position int
	End      int
}

func (field Field) name() string {
	if field.Kind == FieldFact {
		return fmt.Sprintf("facts[%q]", field.FactKey)
	}
	return field.Column
}

type Expression interface {
	span() (int, int)
}

type And struct {
	Left, Right Expression
}

type Or struct {
	Left, Right Expression
}

type Not struct {
	Operand  Expression
	Position int
}

type Has struct {
	Field    Field
	Position int
	End      int
}

type Comparison struct {
	Field    Field
	Semver   bool
	Operator Operator
	Values   []Literal
	Position int
	End      int
}

type MatchAll struct{}

func (expression *And) span() (int, int) {
	start, _ := expression.Left.span()
	_, end := expression.Right.span()
	return start, end
}

func (expression *Or) span() (int, int) {
	start, _ := expression.Left.span()
	_, end := expression.Right.span()
	return start, end
}

func (expression *Not) span() (int, int) {
	_, end := expression.Operand.span()
	return expression.Position, end
}

func (expression *Has) span() (int, int) {
	return expression.Position, expression.End
}

func (expression *Comparison) span() (int, int) {
	return expression.Position, expression.End
}

func (expression *MatchAll) span() (int, int) {
	return 0, 0
}

var columns = map[string]FieldKind{
	"device_id":            FieldText,
	"installation_id":      FieldText,
	"cert_fingerprint":     FieldText,
	"lifecycle":            FieldText,
	"country":              FieldText,
	"os_name":              FieldText,
	"os_version":           FieldText,
	"os_build":             FieldText,
	"dusk_version":         FieldVersion,
	"hardware_class":       FieldText,
	"tenant":               FieldText,
	"locale":               FieldText,
	"hostname":             FieldText,
	"impl":                 FieldText,
	"target_arch":          FieldText,
	"reported_version":     FieldVersion,
	"reported_config_hash": FieldText,
	"credential_kind":      FieldText,
	"credential_ref":       FieldText,
	"credential_issuer":    FieldText,
}

func Columns() []string {
	names := make([]string, 0, len(columns))
	for name := range columns {
		names = append(names, name)
	}
	return names
}
