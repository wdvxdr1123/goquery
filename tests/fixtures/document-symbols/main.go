// Document-symbol fixture: every top-level declaration shape gopls reports.
package documentsymbols

import (
	"io"
	"strings"
)

// Functions.

func Plain(a, b int, c string) (int, error) { return 0, nil }

func Variadic(prefix string, values ...any) {}

func Unnamed(int, *string, chan<- map[string][]byte) {}

func Ptr() *Options { return nil }

// Methods.

func (o *Options) Value() int { return o.n }

func (o Options) Pointer(x, y int) (first, second string) { return "", "" }

func (o Options) Variadic(values ...int) {}

func (Box[T]) Get() T { var zero T; return zero }

func (b *Box[T]) Put(value T) {}

// Types.

type Options struct {
	n                 int
	Name, Description string `json:"name"`
	count             uint8
	Embedded
	*io.LimitedReader
	Nested struct {
		A int
		B []byte
	}
	Tagged int `xml:"tagged"`
	_      [0]byte
}

type Embedded struct{ E int }

type Reader interface {
	Read(p []byte) (n int, err error)
	io.Closer
	~int | ~string
	Comparable() bool
}

type EmptyInterface interface{}

type EmptyStruct struct{}

type Number int

type Alias = Options

type Generic[T any] struct{ V T }

type Box[T any] struct{ V T }

type FuncType func(x int) error

type NestedType []map[string]struct{ Inner int }

// Values and constants.

var Variable int

var Grouped, Second = 1, "two"

var Derived = Variable

var Structured struct {
	Field int
}

var UnnamedType struct{}

const Constant = 1

const Typed int64 = 2

const (
	Alpha = iota
	Beta
	Gamma
	Delta = iota + 5
)
