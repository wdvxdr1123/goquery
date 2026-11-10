package factory

import data "example.test/handles"

func New() *data.Handle              { return &data.Handle{} }
func Generic() data.Box[data.Handle] { return data.Box[data.Handle]{} }
func Alias() data.Alias              { return data.Alias{} }
