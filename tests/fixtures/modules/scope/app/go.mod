module example.test/app
go 1.27
require (
 example.test/dep v0.0.0
 example.test/wrapper v0.0.0
)
replace example.test/dep => ../dep
replace example.test/wrapper => ../wrapper
