module example.test/scoped

go 1.27

require (
	example.test/factory v0.0.0
	example.test/handles v0.0.0
)

replace example.test/factory => ./lib

replace example.test/handles => ./other
