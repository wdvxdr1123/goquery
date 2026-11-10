package lib

type T struct{ Wrong int }
type Identity[X any] = Token[X]
type Token[T any] struct{}
type OtherToken[T any] struct{}
type Echo[T any] struct{}
func (Echo[Element]) Convert(Element) Element { return *new(Element) }
type Mapper[V any] interface { Convert(V) V }
type MapperAlias[V any] = Mapper[V]
type DefinedMapper[V any] Mapper[V]
type Embed[T any] struct { Echo[T] }
type Defined[T any] Embed[T]
type Alias[T any] = Embed[T]
type Layer[T any] struct { Embed[[]T] }
type Repeated[T any] struct{}
func (Repeated[R]) Pair(R, R) R { return *new(R) }
type Independent[A, B any] struct{}
func (Independent[X, Y]) Pair(X, Y) X { return *new(X) }
type Recursive[A any] struct{}
func (Recursive[V]) Pair(V, *V) V { return *new(V) }
type Left[T any] struct{}
func (Left[V]) Mix(V) int { return 0 }
type Right[T any] struct{}
func (Right[V]) Mix(string) V { return *new(V) }
type Grow[T any] struct{}
func (Grow[V]) Cycle(V) []V { return nil }
type Shrink[T any] struct{}
func (Shrink[V]) Cycle([]V) V { return *new(V) }
type Named[T any] struct{}
func (Named[V]) Named(Identity[V]) Identity[V] { return Token[V]{} }
type Visitor[T any] struct{}
func (Visitor[V]) Visit(func(V) V) V { return *new(V) }
type Array[T any] struct{}
func (Array[V]) Array([0x10]V) [16]V { return [16]V{} }
type Reorder[A, B any] struct{}
func (Reorder[Y, X]) Order(Y, X) X { return *new(X) }
type Sequence[T any] struct{}
func (Sequence[V]) Each(...V) V { return *new(V) }
