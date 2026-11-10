package other

type Exported struct{}
func (Exported) Accept(struct { X int }) {}
func (Exported) Hide(struct { x int }) {}
func (Exported) Blank(struct { _ int }) {}
