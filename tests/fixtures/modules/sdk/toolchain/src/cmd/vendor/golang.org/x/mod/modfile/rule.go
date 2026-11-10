package modfile
import "golang.org/x/mod/semver"
func Parse(s string) int { return semver.Compare(s, s) }
