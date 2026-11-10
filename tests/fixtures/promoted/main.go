package main

import mid "example.test/promoted/mid"

type Single struct { mid.Wrapper }
type A struct { mid.Wrapper }
type B struct { mid.Wrapper }
type Diamond struct { A; B }
type Shadow struct { mid.Wrapper; Run func() }
type Override struct { mid.Wrapper }
func (v Override) Run() {}
type Cycle struct { *Cycle }
func (v Cycle) Own() {}
func Use(a Single, d Diamond, s Shadow, o Override, c Cycle) {
 a.Run()
 d.Run()
 s.Run()
 o.Run()
 c.Own()
}

type Unknown struct { error }
func (v Unknown) Own() {}
func UseUnknown(u Unknown) { u.Own() }
