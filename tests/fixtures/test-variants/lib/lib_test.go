package lib_test

import lib "example.test/variants/lib"

func UseExternally() {
	lib.ExportedForTest()
	_ = lib.ExportedTestType{Field: 1}
	lib.Item{}.TestMethod()
}
