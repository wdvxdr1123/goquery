package other

type Secret interface { hidden() }
type Private struct{}
func (Private) Private(interface { hidden() }) {}
