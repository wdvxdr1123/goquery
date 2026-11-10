package main

import api "example.test/implementation/lib"
import foreign "example.test/implementation/other"

type Good struct{}
func (*Good) Read(p []uint8) (n int, err error) { return 0, nil }
func (Good) Write([]byte) (int, error) { return 0, nil }
type Promoted struct { *Good }
type Ambiguous struct { Good; foreign.ReaderImpl }
type Blocked struct { Good; Read int }
type Wrong struct{}
func (Wrong) Read([]rune) (int, error) { return 0, nil }
type Wrapped struct { api.Sealed }
type PlainEmbed struct { Good }
type Empty interface{}
type EmptyStruct struct{}
type Alias = Good
type InterfaceAlias = api.Reader
type LiteralAlias = interface { Read([]byte) (int, error) }
type Distinct Good
type Both interface { api.Private; foreign.Private }
type BothImpl struct { api.Reading; foreign.ReaderImpl }
type FakeSeal struct{}
func (FakeSeal) Read([]byte) (int, error) { return 0, nil }
func (FakeSeal) seal() {}
type Typed struct{}
func (Typed) Use(api.Alias) api.Payload { return 0 }
type WrongTyped struct{}
func (WrongTyped) Use(foreign.Payload) foreign.Payload { return 0 }
type Variadic struct{}
func (Variadic) Log(string, ...uint8) {}
type Slice struct{}
func (Slice) Log(string, []byte) {}
type Callback struct{}
func (Callback) Call(func(x api.Payload, y api.Payload) (api.Payload, error)) {}
type Array struct{}
func (Array) Array([16]uint8) {}
type WrongArray struct{}
func (WrongArray) Array([17]byte) {}
type Channel struct{}
func (Channel) Pull(<-chan api.Payload) chan<- api.Payload { return nil }
type WrongChannel struct{}
func (WrongChannel) Pull(chan api.Payload) chan api.Payload { return nil }
type Mapped struct{}
func (Mapped) Lookup(map[string]*api.Payload) []api.Payload { return nil }
type WrongMap struct{}
func (WrongMap) Lookup(map[string]*foreign.Payload) []foreign.Payload { return nil }
type NamedFunction struct{}
func (NamedFunction) Invoke(api.NominalFunc) {}
type AnonymousFunction struct{}
func (AnonymousFunction) Invoke(func()) {}
type DefinedPointer *Good
type PointerAlias = *api.Reader
type ConcretePointerAlias = *Good
type DefinedReader api.Reader
type IntGetter interface { Get() int }
type IntBox struct { api.Box[int] }
type Anonymous struct{}
func (Anonymous) Accept(struct { X int }) {}

var _ api.ReadWriter = (*Good)(nil)
var _ api.Sealed = Wrapped{}
var _ Both = BothImpl{}
func main() {}
