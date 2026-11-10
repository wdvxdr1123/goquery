package lib_test

type ExternalReader struct{}
func (ExternalReader) Read([]byte) (int, error) { return 0, nil }
