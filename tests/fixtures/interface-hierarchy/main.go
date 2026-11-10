package main

import "example.test/interfacehierarchy/lib"
import "example.test/interfacehierarchy/other"
import "example.test/interfacehierarchy/shadow"

type Good struct{}
func (Good) Use(interface { lib.A }) {}
func (Good) Union(interface { B(int) string; A() }) {}
func (Good) Duplicate(interface { A() }) {}
func (Good) Int(interface { Get() int }) {}
func (Good) String(interface { Get() string }) {}
func (Good) Empty(any) {}
func (Good) Fail(interface { Error() string }) {}
func (Good) Nested(interface { Callback(func(interface { lib.A }) struct { X int }) }) {}
func (Good) Recursive(interface { Next() lib.Node }) {}
func (Good) Nominal(lib.A) {}
type ViaPrivate struct{}
func (ViaPrivate) Private(interface { lib.Secret }) {}
func (ViaPrivate) Both(interface { lib.Secret }) {}
type Different struct{}
func (Different) Use(interface { B() }) {}
func (Different) Union(interface { A() }) {}
func (Different) Int(interface { Get() string }) {}
func (Different) Nested(interface { Callback(func(interface { B() }) struct { X int }) }) {}
func (Different) Recursive(interface { Next() lib.A }) {}
func (Different) Nominal(interface { A() }) {}
type LocalPrivate struct{}
func (LocalPrivate) Private(interface { hidden() }) {}
type BothPrivate interface { Both(interface { lib.Secret; other.Secret }) }
type BothPrivateImpl struct{}
func (BothPrivateImpl) Both(interface { other.Secret; lib.Secret }) {}

var _ lib.Single = Good{}
var _ lib.Union = Good{}
var _ lib.Duplicate = Good{}
var _ lib.Private = ViaPrivate{}
var _ lib.TwoPrivate = ViaPrivate{}
var _ BothPrivate = BothPrivateImpl{}
var _ lib.Int = Good{}
var _ lib.Int = lib.GenericImpl[int]{}
var _ lib.String = Good{}
var _ lib.EmptyWant = Good{}
var _ lib.Failure = Good{}
var _ shadow.Want = shadow.Impl{}
var _ shadow.Want = Good{}
var _ lib.Nested = Good{}
var _ lib.Recursive = Good{}
var _ lib.Nominal = Good{}
var _ lib.Pair = lib.Repeated[int]{}
func main() {}
