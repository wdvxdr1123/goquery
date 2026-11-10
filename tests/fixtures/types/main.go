package main

import lib "example.test/types/lib"

type Child struct { *lib.Base; Named lib.Other; Nested struct { lib.Other } }
type Grand struct { Child }
type Reader interface { lib.Reader; Close() }
type Alias = lib.Base
