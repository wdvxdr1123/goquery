package lib

type hidden struct{ Value int }
type other struct{ Value int }
type Alias = hidden
type Public struct{ Item *hidden }
type Box[T any] struct{ Value T }
type GenericAlias[T any] = Box[T]
type Callback func() *hidden
var Seed = New()
func New() *hidden { return &hidden{} }
func Pair() (*hidden, Alias) { return New(), hidden{} }
func Duplicate() (*hidden, *hidden) { return New(), New() }
func (p Public) Get() *hidden { return p.Item }
func Generic[T any](v T) Box[T] { return Box[T]{Value:v} }
