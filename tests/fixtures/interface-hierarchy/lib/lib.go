package lib

type Text = string
type A interface { A() }
type B interface { B(int) string }
type Secret interface { hidden() }
type Generic[T any] interface { Get() T }
type GenericAlias[T any] = Generic[T]
type Defined[T any] Generic[T]
type Empty interface{}
type EmptyAlias = any
type Node interface { Next() Node }
type Single interface { Use(interface { A() }) }
type Union interface { Union(interface { A; B }) }
type Duplicate interface { Duplicate(interface { A; interface { A() } }) }
type Private interface { Private(interface { hidden() }) }
type PrivateImpl struct{}
func (PrivateImpl) Private(interface { hidden() }) {}
type TwoPrivate interface { Both(interface { Secret; interface { hidden() } }) }
type Int interface { Int(interface { GenericAlias[int] }) }
type String interface { String(interface { Defined[string] }) }
type EmptyWant interface { Empty(interface { Empty; EmptyAlias }) }
type Failure interface { Fail(interface { error }) }
type Nested interface { Nested(interface { Callback(func(interface { A() }) struct { X int }) }) }
type Recursive interface { Recursive(interface { Node }) }
type Nominal interface { Nominal(A) }
type GenericImpl[T any] struct{}
func (GenericImpl[V]) Int(interface { Get() V }) {}
type Repeated[T any] struct{}
func (Repeated[V]) Pair(interface { Left() V; Right() V }) {}
type Pair interface { Pair(interface { Left() int; Right() int }) }
type Mixed interface { Pair(interface { Left() int; Right() string }) }
