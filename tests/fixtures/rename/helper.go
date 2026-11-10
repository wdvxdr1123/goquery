package rename

import libalias "example.test/rename/lib"

func helperValue() int {
	record := Record{Name: "helper"}
	record.Method()
	_ = record.hidden
	return ExportedVar + ExportedConst + libalias.Value
}

func sameName() int {
	record := Record{}
	record.Method()
	return record.Inner.Value
}
