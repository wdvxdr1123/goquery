package corpus

func Empty()
func Grouped(a, b int, c string)
func Unnamed(int, string, pkg.T, *T, []int, T[P])
func Arrays(a [N]int, b []T, c [len("x")]byte)
func Channels(a <-chan T, b chan<- T, c chan T)
func Variadic(a int, rest ...T)
func UnnamedVariadic(int, ...T)
func Results(a T) (first, second T, err error)
func Result() *pkg.T
func Nested(f func(a, b int) (x T), values map[string]func(...T) error)
func Aggregate(v struct{ X int }, constraint interface{ M() })
func (receiver *T) Method(a T) (result T)
func (*T[P]) UnnamedReceiver()
func Generic[T, U any, V ~int | ~string](v T, other U) V
func Constraint[T interface{ ~int | ~string; M() }](v T)
func TypeArguments(T[P], pkg.T[P, Q], *T[P])
func NamedArrays(T [P]Q, U []V)
