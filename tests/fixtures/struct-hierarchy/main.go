package main

import "example.test/structhierarchy/lib"
import "example.test/structhierarchy/other"

type Good struct{}
func (Good) Accept(struct { X int }) {}
type Alias = struct { X int }
type ThroughAlias struct{}
func (ThroughAlias) Accept(Alias) {}
type Nominal struct{}
func (Nominal) Accept(lib.Named) {}
type WrongName struct{}
func (WrongName) Accept(struct { Y int }) {}
type WrongType struct{}
func (WrongType) Accept(struct { X string }) {}
type LocalPrivate interface { Hide(struct { x int }) }
type Local struct{}
func (Local) Hide(struct { x int }) {}
func (Local) Blank(struct { _ int }) {}
type ViaPrivateAlias struct{}
func (ViaPrivateAlias) Hide(lib.PrivateShape) {}
func (ViaPrivateAlias) Blank(lib.BlankShape) {}
type Tags struct{}
func (Tags) Tagged(struct { X int "json:\"\x78\"" }) {}
func (Tags) Bytes(struct { X int "\377" }) {}
func (Tags) EmptyTag(struct { X int `` }) {}
type WrongTags struct{}
func (WrongTags) Tagged(struct { X int `json:"y"` }) {}
func (WrongTags) Bytes(struct { X int "\xfe" }) {}
func (WrongTags) EmptyTag(struct { X int `x` }) {}
type Grouped struct{}
func (Grouped) Grouped(struct { X int; Y int }) {}
type Reordered struct{}
func (Reordered) Grouped(struct { Y int; X int }) {}
type Nested struct{}
func (Nested) Nested(struct { Inner struct { X int }; Values map[string][]*lib.Token }) {}
type Embedded struct{}
func (Embedded) Embed(struct { lib.Token }) {}
func (Embedded) Pointer(struct { *lib.Token }) {}
func (Embedded) GenericEmbed(struct { lib.Box[int] }) {}
type Explicit struct{}
func (Explicit) Embed(struct { Token lib.Token }) {}
func (Explicit) Pointer(struct { Token *lib.Token }) {}
func (Explicit) GenericEmbed(struct { Box lib.Box[int] }) {}
type AliasEmbed struct{}
func (AliasEmbed) Embed(struct { lib.TokenAlias }) {}
type Generic[T any] struct{}
func (Generic[V]) Accept(struct { X V }) {}
type Empty struct{}
func (Empty) Empty(struct{}) {}

var _ lib.Record = Good{}
var _ lib.Record = ThroughAlias{}
var _ lib.Record = other.Exported{}
var _ lib.Private = ViaPrivateAlias{}
var _ lib.Blank = ViaPrivateAlias{}
var _ lib.Tagged = Tags{}
var _ lib.Bytes = Tags{}
var _ lib.EmptyTag = Tags{}
var _ lib.Grouped = Grouped{}
var _ lib.Nested = Nested{}
var _ lib.Embedded = Embedded{}
var _ lib.Pointer = Embedded{}
var _ lib.GenericEmbed = Embedded{}
var _ lib.ValueInt = lib.GenericValue[int]{}
var _ lib.RepeatInt = lib.Repeated[int]{}
var _ lib.Empty = Empty{}
func main() {}
