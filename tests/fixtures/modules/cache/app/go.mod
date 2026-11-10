module example.test/app

go 1.25.0
require (
 example.test/Upper v1.0.0
 example.test/shared v1.2.0
 example.test/old v0.0.0
)
replace example.test/old => example.test/New v2.0.0+incompatible
