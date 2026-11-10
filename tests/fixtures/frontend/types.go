package corpus

type _ = pkg.T[A, *B]
type _ = [Count]map[Key][]*Value
type _ = struct{ a, b T; *pkg.Base[A]; Callback func(x T) (result U); Tag string `json:"tag"` }
type _ = interface{ Method(a, b T) (U, error); Other; pkg.Base[A]; ~int | ~string }
type _ = struct{ Nested struct{ Field T }; I interface{ M([]T) map[K]V } }
type _ = func(T, *pkg.U, ...V) (first, second W)
type _ = chan<- <-chan T
type _ = interface{ ~[]T | map[K]V; comparable }
type _ = struct{ T; A [N]B; Generic[P] }
type _ = [...]T
