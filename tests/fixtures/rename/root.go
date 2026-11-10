// Package rename exercises symbol renaming: [Record], [Point] and [Generic].
package rename

import (
	"example.test/rename/lib"
	alias "example.test/rename/other"
)

var ExportedVar = 1

var shadowed = 2

const ExportedConst = 3

// Record documents [Point] and [Nested] members.
type Record struct {
	Name   string
	hidden int
	Inner  Nested
	*Point
}

type Nested struct {
	Value int
}

type Point struct {
	X, Y int
}

func (r *Record) Method() { r.hidden = 1 }

func (r *Record) hiddenMethod() { r.Method() }

type Renamer interface {
	Method()
}

type Wrapper struct {
	Record
}

func (w Wrapper) Use() {
	w.Method()
	w.Name = "wrapper"
}

// Local returns [Record] data through [ExportedVar] and [ExportedConst].
func Local() int {
	value := 1
	{
		value := 2
		_ = value
	}
	return value
}

func Param(value int) int {
	value += 1
	return value
}

func Results() (result int) {
	result = 1
	return
}

func Closures() {
	total := 0
	for _, item := range []int{1, 2} {
		total += item
	}
	for index := 0; index < total; index++ {
		total += index
	}
	if asserted, ok := any(total).(int); ok {
		total += asserted
	}
	switch typed := any(total).(type) {
	case int:
		total += typed
	}
	recv := &Record{Name: "record"}
	recv.hiddenMethod()
	composed := Wrapper{Record: Record{}}
	composed.Method()
	_ = lib.Value
	_ = alias.Value
	_ = len("builtin")
loop:
	for {
		break loop
	}
}

func Generic[T any](item T) T {
	return item
}

func Calls() {
	_ = Generic(1)
	_ = ExportedVar
	_ = ExportedConst
	_ = lib.Value
	_ = alias.Value
}
