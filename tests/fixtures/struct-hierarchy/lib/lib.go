package lib

type Record interface { Accept(struct { X int }) }
type Private interface { Hide(struct { x int }) }
type Blank interface { Blank(struct { _ int }) }
type PrivateShape = struct { x int }
type BlankShape = struct { _ int }
type PrivateImpl struct{}
func (PrivateImpl) Hide(struct { x int }) {}
func (PrivateImpl) Blank(struct { _ int }) {}
type Tagged interface { Tagged(struct { X int `json:"x"` }) }
type Bytes interface { Bytes(struct { X int "\xff" }) }
type EmptyTag interface { EmptyTag(struct { X int }) }
type Grouped interface { Grouped(struct { X, Y int }) }
type Nested interface { Nested(struct { Inner struct { X int }; Values map[string][]*Token }) }
type Token struct{}
type TokenAlias = Token
type Embedded interface { Embed(struct { Token }) }
type Pointer interface { Pointer(struct { *Token }) }
type Box[T any] struct{}
type GenericEmbed interface { GenericEmbed(struct { Box[int] }) }
type GenericValue[T any] struct{}
func (GenericValue[V]) Value(struct { Value V `json:"value"` }) {}
type ValueInt interface { Value(struct { Value int `json:"value"` }) }
type ValueString interface { Value(struct { Value string `json:"value"` }) }
type Repeated[T any] struct{}
func (Repeated[V]) Repeat(struct { X, Y V }) {}
type RepeatInt interface { Repeat(struct { X int; Y int }) }
type RepeatMixed interface { Repeat(struct { X int; Y string }) }
type Empty interface { Empty(struct{}) }
type Named struct { X int }
type Recursive[T any] struct{}
func (Recursive[V]) Cycle(V) struct { Next V } { return struct { Next V }{} }
type RecursiveWant[T any] interface { Cycle(struct { Next T }) T }
