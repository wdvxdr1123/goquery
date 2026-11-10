package lib

type ExportedTestType struct{ Field int }

func ExportedForTest()   {}
func (Item) TestMethod() {}
func UseInternally()     { ExportedForTest() }
