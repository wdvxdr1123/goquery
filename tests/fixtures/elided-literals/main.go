package main
import "example.test/elided/other"
type Point struct { X int; Y int }
type Alias = Point
type Rows[T any] []T
type Bucket map[string]Point
type Key struct { ID int }
const X = 2
const Index = 0
func pair() (int, int) { return 0, 0 }
func Probe() {
 _, n := pair()
 _ = n
 var _ = n
 const _ = 3
 type _ int
 rows := Rows[Point]{{X: 1, Y: 2}, {X: 3}}
 arrays := [2]Point{Index: {X: 4}}
 nested := [][]Point{{{X: 5}}}
 maps := map[Key]Point{{ID: 1}: {X: 6}}
 ptrs := []*Point{{X: 7}}
 data := []Bucket{{"name": {X: 8}}}
 expr := []map[int]int{{X: n}}
 localKey := X
 expr2 := []map[int]int{{localKey: n}}
 aliases := []Alias{{X: 9}}
 foreign := []other.Point{{X: 10}}
 _ = rows; _ = arrays; _ = nested; _ = maps; _ = ptrs; _ = data
 _ = expr; _ = expr2; _ = aliases; _ = foreign
}
