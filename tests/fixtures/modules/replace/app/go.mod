module "example.test/app"

go 1.27

require (
 example.test/dep v0.0.0
)
replace (
 example.test/dep => "../dep source" // local sources, not the declared module ID
)
