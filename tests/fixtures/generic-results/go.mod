module example.test/results

go 1.27

require (
	example.test/resultlib v0.0.0
	example.test/resultdata v0.0.0
)

replace example.test/resultlib => ./lib
replace example.test/resultdata => ./other
