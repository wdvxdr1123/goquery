package main

import lib "example.test/calls/lib"

func Caller() {
 lib.Public()
 lib.Public()
 Local()
 f := Local
 f()
}
func Local() { Caller() }

func Consume(v int) int { return v }
func Nested() {
 _ = Consume(Consume(1))
 (Local)()
 defer lib.Public()
 go lib.Public()
 func() { lib.Public() }()
}
func Unicode() { _ = "🙂"; lib.Public() }; func SameLine() { lib.Public() }

func Bindings(ch chan func(), value any) {
 Local()
 { Local := func() {}; Local(); Local, n := Local, 1; _ = n; Local() }
 Local()
 if Local := func() {}; true { Local() } else { Local() }
 for Local := range ch { Local() }
 select { case Local := <-ch: Local(); default: Local() }
 switch Local := value.(type) { case func(): Local(); default: _ = Local }
 Local()
}

func Factory() func() { return func() {} }
func Initializer() { Factory := Factory(); Factory() }
func Iter() chan func() { return nil }
func RangeInitializer() { for Iter := range Iter() { Iter() } }
