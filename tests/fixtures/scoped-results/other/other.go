package handles

type Handle struct {
	Public int
	secret int
	Next   *Handle
}
type Alias = Handle
type Box[T any] struct{ Value T }
type hidden struct {
	Public int
	secret int
}

func (h Handle) Read(name string) string  { return name }
func (h *Handle) private()                {}
func (h *Handle) Hidden() *hidden         { return &hidden{} }
func (h *hidden) Read(name string) string { return name }
func (h *hidden) private()                {}
func (b Box[T]) Get() T                   { return b.Value }
