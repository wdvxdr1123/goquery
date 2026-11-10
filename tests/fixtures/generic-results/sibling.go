package main

import alternate "example.test/resultlib"
import model "example.test/resultdata"

func Local() alternate.Box[model.Item] { return alternate.Box[model.Item]{} }
