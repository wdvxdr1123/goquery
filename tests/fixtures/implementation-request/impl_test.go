package request

// sameTestImpl implements the production package's private interface from an
// internal test variant.
type sameTestImpl struct{}

func (sameTestImpl) Read(p []byte) (int, error) { return 0, nil }
func (sameTestImpl) seal()                      {}

var _ Reader = sameTestImpl{}
var _ private = sameTestImpl{}
