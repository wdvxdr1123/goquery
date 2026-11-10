package scoped

import factory "example.test/factory"

type Handle struct{ Wrong int }

func Probe() {
	_ = factory.New().Read("chain")
	local := factory.New()
	_ = local.Read("local")
	_ = factory.New().Next.Read("field")
	_ = factory.Generic().Get().Read("generic")
	_ = factory.Alias().Read("alias")
	_ = factory.New().Hidden().Read("hidden")
	_ = factory.New().Public
	_ = factory.New().Hidden().Public
	data := 1
	_ = data
	_ = local.Read("shadow")
}
