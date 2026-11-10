package shadow

import "example.test/interfacehierarchy/lib"

type string int
type Want interface { Fail(interface { error }) }
type Impl struct{}
func (Impl) Fail(interface { Error() lib.Text }) {}
type Wrong struct{}
func (Wrong) Fail(interface { Error() string }) {}
