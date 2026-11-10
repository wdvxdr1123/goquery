package resultlib

import foreign "example.test/resultdata"

type Box[T any] struct { Value T; Next *Box[T] }
func (b Box[T]) Get() T { return b.Value }
type Plain struct { Value Box[foreign.Item] }
func (Plain) Get() Box[foreign.Item] { return Box[foreign.Item]{} }
func Make() Box[foreign.Item] { return Box[foreign.Item]{} }
func Nested() Box[Box[foreign.Item]] { return Box[Box[foreign.Item]]{} }
func Sliced() Box[[]*foreign.Item] { return Box[[]*foreign.Item]{} }
func Mapped() Box[map[string]foreign.Item] { return Box[map[string]foreign.Item]{} }
func Pointer() *Box[foreign.Item] { return &Box[foreign.Item]{} }
func Function() Box[func(foreign.Item) foreign.Item] { return Box[func(foreign.Item) foreign.Item]{} }
func FromAlias() Box[foreign.Alias] { return Box[foreign.Alias]{} }
