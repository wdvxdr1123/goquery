package sub

import request "example.test/implementation-request"

// SubReader implements exported interfaces from another package.
type SubReader struct{}

func (SubReader) Read(p []byte) (int, error) { return len(p), nil }
func (SubReader) Greet(name string) string   { return name }
func (*SubReader) Farewell() string          { return "bye" }

// hidden is implemented only inside this package.
type hidden interface {
	mark()
}

// HiddenImpl satisfies the private interface of its own package.
type HiddenImpl struct{}

func (HiddenImpl) mark() {}

var _ request.Reader = SubReader{}
var _ request.Base = SubReader{}
var _ hidden = HiddenImpl{}
