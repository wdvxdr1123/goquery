package other

type Payload int
type Private interface { seal() }
type ReaderImpl struct{}
func (ReaderImpl) Read([]byte) (int, error) { return 0, nil }
func (ReaderImpl) seal() {}
