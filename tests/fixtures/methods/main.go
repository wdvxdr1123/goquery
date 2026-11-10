package main

import lib "example.test/methods/lib"

type T struct{}
func (t T) Run() {}
func Use(v lib.T, local T) {
 v.Run()
 local.Run()
 var ptr *lib.T
 ptr.Run()
 v.hidden()
}
func Shadow() { type T struct{}; var v T; v.Run() }

func Scopes(v lib.T, ch chan func(), value any) {
 v.Run()
 v, n := v, 1
 _ = n
 v.Run()
 { v := func() {}; v() }
 v.Run()
 if v := func() {}; true { v() } else { v() }
 for v := range ch { v() }
 select { case v := <-ch: v(); default: v.Run() }
 switch v := value.(type) { case func(): v(); default: _ = v }
 v.Run()
 { type T lib.T; var x T; x.Run(); T.Run() }
 var p lib.T
 p, x := p, 1
 _ = x
 (p).Run()
}
func Generic[T interface{ Run() }](v T) { v.Run() }
func SameName(T T) { T.Run() }
func NamedResult() (v lib.T) { v.Run(); return }

func Grouped(a, b *lib.T) (first, second lib.T) {
 a.Run(); b.Run(); first.Run(); second.Run(); return
}
func Variadic(values ...lib.T) { values.Run() }
type Box[A interface{ Run() }] struct{}
func (b Box[T]) GenericReceiver(v T) { v.Run() }
func (v T) Receiver() { v.Run() }
