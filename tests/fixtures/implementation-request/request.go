// Package request is the interface/implementation differential fixture.
// It exercises value and pointer method sets, embedding, generics, aliases,
// cross-package implementations and test variants. Expected relations are
// recorded by the feature differential, not by this source file.
package request

// Greeter is implemented by value and pointer receivers and in other packages.
type Greeter interface {
	Greet(name string) string
}

// Base embeds Greeter and adds another method.
type Base interface {
	Greeter
	Farewell() string
}

// Reader is implemented across packages, including test variants.
type Reader interface {
	Read(p []byte) (int, error)
}

// Stringer adds a second supertype for concrete types.
type Stringer interface {
	String() string
}

// Simple has a value receiver Greet and pointer receiver Farewell.
type Simple struct{}

func (Simple) Greet(name string) string { return "hello " + name }
func (Simple) String() string           { return "simple" }
func (*Simple) Farewell() string        { return "bye" }

// PointerOnly receives Greeter only through its pointer method set.
type PointerOnly struct{}

func (*PointerOnly) Greet(name string) string { return name }

// Alias renames Simple without changing the implementation identity.
type Alias = Simple

// Generic implements Greeter for every instantiation.
type Generic[T any] struct{ Value T }

func (Generic[T]) Greet(name string) string { return name }

// GenericInterface is implemented by GenericImpl under unification.
type GenericInterface[T any] interface {
	Use(value T)
}

type GenericImpl[T any] struct{}

func (GenericImpl[T]) Use(value T) {}

// noMethods participates in no structural relation.
type noMethods struct{}

// Variadic and Slice distinguish variadic from slice signatures.
type Variadic struct{}

func (Variadic) Log(format string, args ...any) {}

type Slice struct{}

func (Slice) Log(format string, args []any) {}

// private is implemented inside this package and its same-package tests only.
type private interface {
	seal()
}

// LocalFriend implements the unexported interface in the production package.
type LocalFriend struct{}

func (LocalFriend) seal()          {}
func (LocalFriend) Read(p []byte) (int, error) {
	return len(p), nil
}
