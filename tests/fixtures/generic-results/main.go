package main

import api "example.test/resultlib"
import data "example.test/resultdata"

func Probe(p api.Plain) {
	a := api.Make()
	_ = a.Value.Leaf
	_ = api.Make().Value.Leaf
	_ = api.Nested().Value.Value.Leaf
	_ = api.Sliced().Value[0].Leaf
	_ = api.Mapped().Value["x"].Leaf
	_ = api.Pointer().Next.Value.Leaf
	_ = api.FromAlias().Value.Leaf
	_ = p.Value.Value.Leaf
	_ = p.Get().Value.Leaf
	_ = Local().Value.Leaf
	_ = a.Get().Leaf
	_ = api.Function().Value(data.Item{}).Leaf
	_ = a.Value.Name
}

func main() {}
