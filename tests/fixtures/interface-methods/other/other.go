package other
type hidden interface { Read([]byte) (int, error); seal() }
type Exported interface { hidden }
