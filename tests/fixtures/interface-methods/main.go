package main
import "example.test/interfacemethods/other"
type A interface { Read(p []byte) (n int, err error) }
type B interface { Read([]uint8) (int, error) }
type Combined interface { A; B }
type Reverse interface { B; A }
type Diamond interface { Combined; A }
type Branch struct { B }
type Shallow struct { Combined; Branch }
type Ambiguous struct { Combined; B }
type Slot[T any] interface { Put(T) T }
type Left[T any] interface { Slot[T] }
type Right[T any] interface { Slot[T] }
type Dual[T any] interface { Left[T]; Right[T] }
type StructDual struct { Dual[int] }
type IntBranch struct { Slot[int] }
type StringBranch interface { Slot[string] }
type Mixed struct { IntBranch; StringBranch }
type Remote interface { other.Exported; Combined }
func Probe(c Combined, r Reverse, d Diamond, s Shallow, a Ambiguous, g Dual[int], h StructDual, m Mixed, q Remote) {
 c.Read(nil)
 r.Read(nil)
 d.Read(nil)
 s.Read(nil)
 _ = a.Combined
 g.Put(1)
 h.Put(2)
 m.Put("x")
 q.Read(nil)
}
