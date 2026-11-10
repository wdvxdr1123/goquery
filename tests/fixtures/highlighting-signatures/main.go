package main
func Bounds() (x, y int) { return }
func Visit(iter func(_ int, v int) bool) { _ = iter }
func Probe() {
 f := func(_ int, v int) (x int) { return v }
 _ = f
}
type Walker interface { Walk(func(_ int, v int) (ok bool)) }
