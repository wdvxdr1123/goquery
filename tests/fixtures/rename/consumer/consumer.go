package consumer

import "example.test/rename"

func Use(record rename.Record) {
	record.Method()
	record.Name = "consumer"
	_ = record.Inner.Value
}

type Impl struct {
	rename.Record
}

func (i Impl) Method() {}

type Holder struct {
	Rename rename.Renamer
}

func Call(holder Holder) {
	holder.Rename.Method()
}
