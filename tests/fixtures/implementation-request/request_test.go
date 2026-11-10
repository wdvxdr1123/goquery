package request_test

import (
	"example.test/implementation-request"
)

// externalImpl implements a request interface from an external test package.
type externalImpl struct{}

func (externalImpl) Read(p []byte) (int, error) { return 0, nil }

var _ request.Reader = externalImpl{}
