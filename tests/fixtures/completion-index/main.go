package main
type Block struct { ID int; Succs []Edge }
type Edge struct { b *Block }
type Cache struct{}
func (*Cache) allocBlockSlice(n int) []*Block { return nil }
type Func struct { Cache *Cache }
type Map map[int]*Block
func ptrMap() *Map { return nil }
func blocks() []*Block { return nil }
func array() [2]*Block { return [2]*Block{} }
func Probe(f *Func, bid int, m Map, a *[2]*Block) {
 idToBlock := f.Cache.allocBlockSlice(1)
 b := idToBlock[bid]
 _ = b.Succs
 for _, s := range b.Succs { _ = s.b.ID }
 v, ok := m[0]; _ = ok; _ = v.ID
 _ = blocks()[0].ID
 _ = array()[0].ID
 _ = a[0].ID
}
