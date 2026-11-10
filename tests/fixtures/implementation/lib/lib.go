package lib

type Payload int
type Alias = Payload
type Reader interface { Read([]byte) (int, error) }
type Equivalent interface { Read(p []uint8) (n int, err error) }
type Writer interface { Write([]byte) (int, error) }
type ReadWriter interface { Reader; Writer }
type Private interface { seal() }
type Sealed interface { Reader; Private }
type Reading struct{}
func (*Reading) Read([]byte) (int, error) { return 0, nil }
func (Reading) seal() {}
type Typed interface { Use(Payload) Payload }
type Variadic interface { Log(string, ...byte) }
type Callback interface { Call(func(a, b Payload) (Payload, error)) }
type Array interface { Array([0x10]byte) }
type Channel interface { Pull(<-chan Payload) chan<- Payload }
type Mapped interface { Lookup(map[string]*Payload) []Payload }
type NominalFunc func()
type NamedFunction interface { Invoke(NominalFunc) }
type Box[T any] struct{}
func (Box[T]) Get() T { return *new(T) }
type Anonymous interface { Accept(struct { X int }) }
