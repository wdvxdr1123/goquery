package corpus

type _ = struct{ A, B }
type _ = struct{ A ...T }
type _ = struct{ []T }
type _ = struct{ **T }
type _ = struct{ A T, B U }
type _ = interface{ M(a T, []U) }
type _ = interface{ ~int | }
type _ = interface{ Method[T any]() }
type _ = map[T]
type _ = pkg.
type _ = [N
type _ = struct{ a [...]T }
