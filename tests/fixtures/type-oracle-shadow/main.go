package typeoracleshadow

type int struct{}

func (int) Phantom()            {}
func Identity[T any](value T) T { return value }

func Query(value int) {
	literal := 1
	copyLiteral := literal
	genericLiteral := Identity(1)
	typed := Identity(value)
	_ = []any{literal, copyLiteral, genericLiteral, typed}
}
