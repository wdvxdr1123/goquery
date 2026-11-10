package unrelated

type Base struct{}
func (v Base) Run() {}
func Use(v Base) { v.Run() }
