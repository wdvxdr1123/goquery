package lib

type Item struct{}

func Public()        {}
func (Item) Method() {}
func TestPublic()    {}
