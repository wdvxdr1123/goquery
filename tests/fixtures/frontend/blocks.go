package corpus

func basic() { a := 1; a += 2; a++; return }
func nested() { var a int; const b = 2; type T struct{ X int }; _ = T{X: b}; { a := 3; _ = a }; _ = a }
func calls() { go work(); defer cleanup(); ch <- value; return a, b }
func multiple() { a, b := first(), second(); a, b = b, a; a <<= 1 }
func empty() { { }; return }
func aliases() { type T = []int; var values T = T{1, 2}; const C int = 3; _ = values }
func condition() { if x := work(); x > 0 { return x } else if other { return 2 } else { return 3 } }
func loops() { for i := 0; i < n; i++ { work(i) }; for ready { work() }; for ;; { return } }
func choices() { switch x { case 1, 2: work(); default: return }; switch { case ready: return } }
func compositeCondition() { if equal(T{}, other) { work() }; if (T{}) == other { work() } }
func ranges() { for i, v := range values { use(i, v) }; for i = range values { use(i) }; for range count { work() } }
func branches() { Outer: for { if done { break Outer }; continue }; goto End; End: }
func falling() { switch x { case 1: work(); fallthrough; default: return } }
func selection() { select { case ch <- value: work(); case x, ok := <-ch: use(x, ok); case (<-other): return; default: return } }
func emptySelection() { select {} }
func declarations() { var ( a int; b = 2 ); const ( A = iota; B ); type ( T = int; U struct{ X T } ); _ = b }
// Syntax accepts a general receive expression; channel validation is semantic.
func expressionCommunication() { select { case work(): return } }
func typeSwitch() { switch value := input.(type) { case *Thing, []int: use(value); case nil: return; default: use(value) } }
func typeSwitchInit() { switch input := work(); value := input.(type) { case map[string]int: use(value); case func() int: return } }
func bareTypeSwitch() { switch input.(type) { case chan int: work(); case interface{ Read() }: work() } }
// Like go/parser, retain syntactic case expressions for later type validation.
func syntacticCaseType() { switch input.(type) { case 1 + 2: return } }
func literalHeaderElements() { for _, v := range []struct{ P *T }{{P: &T{}}} { use(v) }; if equal([]T{{X: T{}}}, nil) { use() } }
func localGeneric() { type Box[T any] struct{ Value T }; type Pair[K comparable, V any] map[K]V; type Ignore[_ any] struct{}; var b Box[int]; _ = b }
func arrayOrParameters() { type Array [n]int; type Product [P*C]int; type Pointer[T *C,] []T; var a Array; _ = a }
