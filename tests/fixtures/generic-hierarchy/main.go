package main

import api "example.test/generichierarchy/lib"

type IntMapper interface { Convert(int) int }
type StringMapper interface { Convert(string) string }
type SliceMapper interface { Convert([]int) []int }
type BadMapper interface { Convert(int) string }
type Concrete struct{}
func (Concrete) Convert(int) int { return 0 }
type IntPair interface { Pair(int, int) int }
type BadPair interface { Pair(int, string) int }
type GenericPair[T any] interface { Pair(T, T) T }
type IndependentPair[A, B any] interface { Pair(A, B) A }
type RecursivePair[T any] interface { Pair(T, *T) T }
type Left[T any] interface { Mix(T) int }
type Right[T any] interface { Mix(string) T }
type Grow[T any] interface { Cycle(T) []T }
type Shrink[T any] interface { Cycle([]T) T }
type Named interface { Named(api.Token[int]) api.Token[int] }
type WrongNamed interface { Named(api.OtherToken[int]) api.OtherToken[int] }
type Visitor interface { Visit(func(int) int) int }
type BadVisitor interface { Visit(func(int) string) int }
type Array interface { Array([16]int) [0x10]int }
type WrongArray interface { Array([17]int) [16]int }
type Order interface { Order(int, string) string }
type Sequence interface { Each(...int) int }
type Slice interface { Each([]int) int }
type Fixed struct { api.Echo[int] }

var _ IntMapper = api.Echo[int]{}
var _ IntMapper = api.Defined[int]{}
var _ SliceMapper = api.Layer[int]{}
var _ IntPair = api.Repeated[int]{}
var _ Visitor = api.Visitor[int]{}
var _ Array = api.Array[int]{}
var _ Order = api.Reorder[int, string]{}
func main() {}
