// Syntax acceptance and canonical AST-shape oracle. The `--shapes` mode emits
// only the node kinds modelled by the Rust syntax view, with byte spans, so a
// differential can compare shapes without a second AST format.
package main

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"go/ast"
	"go/build"
	"go/build/constraint"
	"go/parser"
	"go/scanner"
	"go/token"
	"hash/fnv"
	"io"
	"os"
	"sort"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"
)

func main() {
	var sources []string
	if err := json.NewDecoder(os.Stdin).Decode(&sources); err != nil {
		panic(err)
	}
	if len(os.Args) > 1 && os.Args[1] == "--identifier-categories" {
		report := struct {
			Version   string
			Letters   *unicode.RangeTable
			Digits    *unicode.RangeTable
			Uppercase *unicode.RangeTable
		}{unicode.Version, unicode.L, unicode.Digit, unicode.Upper}
		if err := json.NewEncoder(os.Stdout).Encode(report); err != nil {
			panic(err)
		}
		return
	}
	if len(os.Args) > 1 && os.Args[1] == "--match-files" {
		results := make([]bool, len(sources))
		for i, sample := range sources {
			parts := strings.SplitN(sample, "|", 3)
			platform := strings.SplitN(parts[0], "/", 2)
			context := build.Context{GOOS: platform[0], GOARCH: platform[1], Compiler: "gc", BuildTags: []string{"feature"}, ReleaseTags: []string{"go1.27"}}
			source := "package p\n"
			if parts[2] != "" {
				source = "//go:build " + parts[2] + "\n\n" + source
			}
			context.OpenFile = func(string) (io.ReadCloser, error) { return io.NopCloser(strings.NewReader(source)), nil }
			matched, err := context.MatchFile(".", parts[1])
			results[i] = err == nil && matched
		}
		if err := json.NewEncoder(os.Stdout).Encode(results); err != nil {
			panic(err)
		}
		return
	}
	if len(os.Args) > 1 && os.Args[1] == "--token-streams" {
		streams := make([][]tokenSpan, len(sources))
		for i, source := range sources {
			streams[i] = scanTokens([]byte(source))
		}
		if err := json.NewEncoder(os.Stdout).Encode(streams); err != nil {
			panic(err)
		}
		return
	}
	if len(os.Args) > 1 && os.Args[1] == "--file-bodies" {
		reports := make([]fileReport, 0, len(sources))
		for _, name := range sources {
			set := token.NewFileSet()
			file, err := parser.ParseFile(set, name, nil, parser.AllErrors)
			report := fileReport{File: name, Bodies: []bodySpan{}, Headers: []signatureSpan{}, Types: []typeSpan{}}
			source, readErr := os.ReadFile(name)
			if readErr != nil {
				panic(readErr)
			}
			report.Tokens = scanTokens(source)
			if err != nil {
				report.Error = err.Error()
				if errors, ok := err.(scanner.ErrorList); ok {
					report.ErrorCount = len(errors)
				} else {
					report.ErrorCount = 1
				}
			} else {
				for _, decl := range file.Decls {
					if group, ok := decl.(*ast.GenDecl); ok {
						for _, spec := range group.Specs {
							if values, ok := spec.(*ast.ValueSpec); ok {
								for _, value := range values.Values {
									report.Initializers = append(report.Initializers, bodySpan{Start: set.Position(value.Pos()).Offset, End: set.Position(value.End()).Offset})
								}
							}
						}
					}
				}
				ast.Inspect(file, func(node ast.Node) bool {
					var body *ast.BlockStmt
					var typeExpr ast.Expr
					switch node := node.(type) {
					case *ast.TypeSpec:
						typeExpr = node.Type
						if node.TypeParams != nil {
							report.Headers = append(report.Headers, signatureSpan{Start: set.Position(node.TypeParams.Pos()).Offset, End: set.Position(node.TypeParams.End()).Offset, TypeParameters: true})
						}
					case *ast.Field:
						typeExpr = node.Type
					case *ast.ValueSpec:
						typeExpr = node.Type
					case *ast.TypeAssertExpr:
						typeExpr = node.Type
					}
					switch term := typeExpr.(type) {
					case *ast.Ellipsis:
						typeExpr = term.Elt
					case *ast.BinaryExpr:
						typeExpr = nil // Constraint unions use a different grammar.
					case *ast.UnaryExpr:
						if term.Op == token.TILDE {
							typeExpr = nil
						}
					}
					if typeExpr != nil {
						function, method := typeExpr.(*ast.FuncType)
						report.Types = append(report.Types, typeSpan{Start: set.Position(typeExpr.Pos()).Offset, End: set.Position(typeExpr.End()).Offset, Method: method && !function.Func.IsValid()})
					}
					switch node := node.(type) {
					case *ast.FuncDecl:
						body = node.Body
						end := node.End()
						if body != nil {
							end = body.Pos()
						}
						report.Headers = append(report.Headers, signatureSpan{Start: set.Position(node.Pos()).Offset, End: set.Position(end).Offset})
					case *ast.FuncLit:
						body = node.Body
						report.Headers = append(report.Headers, signatureSpan{Start: set.Position(node.Pos()).Offset, End: set.Position(body.Pos()).Offset, Literal: true})
					}
					if body != nil {
						report.Bodies = append(report.Bodies, bodySpan{Start: set.Position(body.Pos()).Offset, End: set.Position(body.End()).Offset})
					}
					return true
				})
			}
			reports = append(reports, report)
		}
		if err := json.NewEncoder(os.Stdout).Encode(reports); err != nil {
			panic(err)
		}
		return
	}
	if len(os.Args) > 1 && os.Args[1] == "--shapes" {
		reports := make([]shapeReport, 0, len(sources))
		for _, name := range sources {
			reports = append(reports, shapeFile(name))
		}
		if err := json.NewEncoder(os.Stdout).Encode(reports); err != nil {
			panic(err)
		}
		return
	}
	if len(os.Args) > 1 && (os.Args[1] == "--negative-files" || os.Args[1] == "--negative-sources") {
		files := os.Args[1] == "--negative-files"
		reports := make([]negativeReport, 0, len(sources))
		for _, source := range sources {
			reports = append(reports, negativeReportOf(source, files))
		}
		if err := json.NewEncoder(os.Stdout).Encode(reports); err != nil {
			panic(err)
		}
		return
	}
	if len(os.Args) > 1 && os.Args[1] == "--recovered-shapes" {
		reports := make([]recoveredReport, 0, len(sources))
		for _, source := range sources {
			reports = append(reports, recoveredShapesOf(source))
		}
		if err := json.NewEncoder(os.Stdout).Encode(reports); err != nil {
			panic(err)
		}
		return
	}
	accepted := make([]bool, len(sources))
	for i, source := range sources {
		if len(os.Args) > 1 && os.Args[1] == "--constraints" {
			expr, err := constraint.Parse("//go:build " + source)
			accepted[i] = err == nil && expr.Eval(func(tag string) bool { return tag == "linux" || tag == "amd64" || tag == "feature" })
		} else if len(os.Args) > 1 && os.Args[1] == "--files" {
			_, err := parser.ParseFile(token.NewFileSet(), "corpus.go", source, parser.AllErrors)
			accepted[i] = err == nil
		} else if len(os.Args) > 1 && os.Args[1] == "--types" {
			_, err := parser.ParseFile(token.NewFileSet(), "corpus.go", "package p\ntype X = "+source, parser.AllErrors)
			accepted[i] = err == nil
		} else if len(os.Args) > 1 && os.Args[1] == "--signatures" {
			_, err := parser.ParseFile(token.NewFileSet(), "corpus.go", "package p\n"+source+" {}", parser.AllErrors)
			accepted[i] = err == nil
		} else if len(os.Args) > 1 && os.Args[1] == "--blocks" {
			_, err := parser.ParseFile(token.NewFileSet(), "corpus.go", "package p\nfunc corpus() "+source, parser.AllErrors)
			accepted[i] = err == nil
		} else {
			_, err := parser.ParseExpr(source)
			accepted[i] = err == nil
		}
	}
	if err := json.NewEncoder(os.Stdout).Encode(accepted); err != nil {
		panic(err)
	}
}

type bodySpan struct {
	Start int `json:"start"`
	End   int `json:"end"`
}
type fileReport struct {
	File         string          `json:"file"`
	Error        string          `json:"error"`
	ErrorCount   int             `json:"error_count"`
	Bodies       []bodySpan      `json:"bodies"`
	Headers      []signatureSpan `json:"headers"`
	Types        []typeSpan      `json:"types"`
	Tokens       []tokenSpan     `json:"tokens"`
	Initializers []bodySpan      `json:"initializers"`
}

// Scanner spellings are normalized only for raw strings, whose literal removes CR.
func scanTokens(source []byte) []tokenSpan {
	var lexer scanner.Scanner
	file := token.NewFileSet().AddFile("corpus.go", -1, len(source))
	lexer.Init(file, source, func(token.Position, string) {}, 0)
	spans := []tokenSpan{}
	for {
		position, kind, literal := lexer.Scan()
		if kind == token.EOF {
			break
		}
		start := file.Offset(position)
		length := len(literal)
		if length == 0 {
			length = len(kind.String())
		}
		inserted := kind == token.SEMICOLON && literal == "\n"
		if inserted {
			length = 0
		}
		if kind == token.STRING && source[start] == '`' {
			length = bytes.IndexByte(source[start+1:], '`') + 2
		}
		spans = append(spans, tokenSpan{kind.String(), start, start + length, inserted})
	}
	return spans
}

type tokenSpan struct {
	Kind     string `json:"kind"`
	Start    int    `json:"start"`
	End      int    `json:"end"`
	Inserted bool   `json:"inserted"`
}

type signatureSpan struct {
	Start          int  `json:"start"`
	End            int  `json:"end"`
	Literal        bool `json:"literal"`
	TypeParameters bool `json:"type_parameters"`
}

type typeSpan struct {
	Start  int  `json:"start"`
	End    int  `json:"end"`
	Method bool `json:"method"`
}

// ---------------------------------------------------------------------------
// --shapes: canonical AST shapes with byte spans.
//
// The projection mirrors the Rust `syntax` view surface: statements, expression
// nodes and a separate flat type-shape sequence. Expression-position type
// subtrees are emitted with the same type-shape kinds as the type section.
// Function-literal bodies are separate components; empty statements, invalid
// nodes and comments are skipped on both sides. Array-length expressions are not
// type nodes. Components above the detail limit carry sorted chunk digests so
// every file is compared without unbounded report memory.
// ---------------------------------------------------------------------------

const (
	// shapeNodeDetailLimit: components with at most this many canonical nodes
	// carry the full node array for detailed diagnostics. Larger components carry
	// sorted chunk digests instead so the whole corpus is compared with bounded
	// report memory; the Rust side rebuilds and digests its own ordered list.
	shapeNodeDetailLimit = 8192
	// shapeChunkNodes is the digest chunk size (ordered canonical nodes).
	shapeChunkNodes = 2048
	// maxShapeNodes is a hard safety bound for pathological generated sources.
	// Reaching it is reported as an explicit truncation, never silently skipped.
	maxShapeNodes = 4_000_000
)

type shapeNode struct {
	Kind  string `json:"kind"`
	Start int    `json:"start"`
	End   int    `json:"end"`
}

// shapeChunk is one digest over a contiguous slice of canonically ordered
// nodes. First/Last bound the chunk for local diagnostics without shipping it.
type shapeChunk struct {
	Count int       `json:"count"`
	Hash  uint64    `json:"hash"`
	First shapeNode `json:"first"`
	Last  shapeNode `json:"last"`
}

type shapeComponent struct {
	Start     int          `json:"start"`
	End       int          `json:"end"`
	Literal   bool         `json:"literal"`
	Method    bool         `json:"method"`
	NodeCount int          `json:"node_count"`
	Nodes     []shapeNode  `json:"nodes"`
	Digests   []shapeChunk `json:"digests"`
	Truncated bool         `json:"truncated"`
}

type declarationShape struct {
	Kind      string `json:"kind"`
	Name      string `json:"name"`
	Start     int    `json:"start"`
	End       int    `json:"end"`
	NameStart int    `json:"name_start"`
	NameEnd   int    `json:"name_end"`
	Generic   bool   `json:"generic"`
	Alias     bool   `json:"alias"`
	Method    bool   `json:"method"`
	Body      bool   `json:"body"`
}

type shapeReport struct {
	File         string             `json:"file"`
	Error        string             `json:"error"`
	Bodies       []shapeComponent   `json:"bodies"`
	Initializers []shapeComponent   `json:"initializers"`
	Types        []shapeComponent   `json:"types"`
	Declarations []declarationShape `json:"declarations"`
	View         viewReport         `json:"view"`
}

// shapeList is a bounded ordered node list: small lists carry the nodes, large
// ones carry sorted chunk digests (same contract as shape components).
type shapeList struct {
	NodeCount int          `json:"node_count"`
	Nodes     []shapeNode  `json:"nodes"`
	Digests   []shapeChunk `json:"digests"`
	Truncated bool         `json:"truncated"`
}

func newShapeList(nodes []shapeNode, truncated bool) shapeList {
	list := shapeList{NodeCount: len(nodes), Nodes: nodes, Truncated: truncated}
	if len(nodes) > shapeNodeDetailLimit && !truncated {
		list.Digests = shapeDigests(nodes)
		list.Nodes = nil
	}
	return list
}

type spanShape struct {
	Start int `json:"start"`
	End   int `json:"end"`
}

type importShape struct {
	Start int    `json:"start"`
	End   int    `json:"end"`
	Name  string `json:"name"`
	Path  string `json:"path"`
}

type commentShape struct {
	Group spanShape `json:"group"`
	First spanShape `json:"first"`
}

type viewReport struct {
	Imports       []importShape  `json:"imports"`
	CommentGroups []commentShape `json:"comment_groups"`
	CommentSpans  []spanShape    `json:"comment_spans"`
	RawStrings    []spanShape    `json:"raw_strings"`
	Groups        shapeList      `json:"groups"`
	Nodes         shapeList      `json:"nodes"`
}

type shapeBuilder struct {
	nodes     []shapeNode
	truncated bool
}

type shapeSpan struct {
	start int
	end   int
}

func (b *shapeBuilder) add(kind string, span shapeSpan) {
	if b.truncated {
		return
	}
	if len(b.nodes) >= maxShapeNodes {
		b.truncated = true
		return
	}
	b.nodes = append(b.nodes, shapeNode{Kind: kind, Start: span.start, End: span.end})
}

// finish builds the report form of one component. Components above the detail
// limit report sorted chunk digests instead of the node array.
func (b *shapeBuilder) finish(start, end int, literal, method bool) shapeComponent {
	component := shapeComponent{
		Start:     start,
		End:       end,
		Literal:   literal,
		Method:    method,
		NodeCount: len(b.nodes),
		Nodes:     b.nodes,
		Truncated: b.truncated,
	}
	if len(b.nodes) > shapeNodeDetailLimit && !b.truncated {
		component.Digests = shapeDigests(b.nodes)
		component.Nodes = nil
	}
	return component
}

// shapeDigests hashes contiguous chunks of the canonically ordered node list.
// The order (kind, start, end) matches the Rust gate's tuple ordering.
func shapeDigests(nodes []shapeNode) []shapeChunk {
	ordered := make([]shapeNode, len(nodes))
	copy(ordered, nodes)
	sort.Slice(ordered, func(i, j int) bool {
		if ordered[i].Kind != ordered[j].Kind {
			return ordered[i].Kind < ordered[j].Kind
		}
		if ordered[i].Start != ordered[j].Start {
			return ordered[i].Start < ordered[j].Start
		}
		return ordered[i].End < ordered[j].End
	})
	chunks := []shapeChunk{}
	for start := 0; start < len(ordered); start += shapeChunkNodes {
		end := min(start+shapeChunkNodes, len(ordered))
		chunk := ordered[start:end]
		chunks = append(chunks, shapeChunk{
			Count: len(chunk),
			Hash:  shapeDigest(chunk),
			First: chunk[0],
			Last:  chunk[len(chunk)-1],
		})
	}
	return chunks
}

// shapeDigest is FNV-1a over kind, a NUL separator and little-endian start/end.
// The Rust gate implements byte-identical hashing without extra dependencies.
func shapeDigest(nodes []shapeNode) uint64 {
	hash := fnv.New64a()
	var buffer [16]byte
	for _, node := range nodes {
		hash.Write([]byte(node.Kind))
		hash.Write([]byte{0})
		binary.LittleEndian.PutUint64(buffer[:8], uint64(node.Start))
		binary.LittleEndian.PutUint64(buffer[8:], uint64(node.End))
		hash.Write(buffer[:])
	}
	return hash.Sum64()
}

func shapeFile(name string) shapeReport {
	report := shapeReport{
		File:         name,
		Bodies:       []shapeComponent{},
		Initializers: []shapeComponent{},
		Types:        []shapeComponent{},
		Declarations: []declarationShape{},
		View: viewReport{
			Imports:       []importShape{},
			CommentGroups: []commentShape{},
			CommentSpans:  []spanShape{},
			RawStrings:    []spanShape{},
		},
	}
	set := token.NewFileSet()
	file, err := parser.ParseFile(set, name, nil, parser.AllErrors|parser.ParseComments)
	if err != nil {
		report.Error = err.Error()
		return report
	}
	source, readErr := os.ReadFile(name)
	if readErr != nil {
		panic(readErr)
	}
	offset := func(position token.Pos) int { return set.Position(position).Offset }
	report.Declarations = declarationShapes(file, offset)
	report.Initializers = initializerShapes(file, offset)
	report.Types = typeShapes(file, offset)
	report.Bodies = bodyShapes(file, offset)
	report.View = viewShapes(file, source, offset)
	return report
}

func declarationShapes(file *ast.File, offset func(token.Pos) int) []declarationShape {
	shapes := []declarationShape{}
	for _, declaration := range file.Decls {
		switch declaration := declaration.(type) {
		case *ast.FuncDecl:
			kind := "func"
			if declaration.Recv != nil {
				kind = "method"
			}
			shapes = append(shapes, declarationShape{
				Kind:      kind,
				Name:      declaration.Name.Name,
				Start:     offset(declaration.Type.Func),
				End:       offset(declaration.End()),
				NameStart: offset(declaration.Name.Pos()),
				NameEnd:   offset(declaration.Name.End()),
				Generic:   declaration.Type.TypeParams != nil,
				Method:    declaration.Recv != nil,
				Body:      declaration.Body != nil,
			})
		case *ast.GenDecl:
			kind := "var"
			switch declaration.Tok {
			case token.CONST:
				kind = "const"
			case token.TYPE:
				kind = "type"
			}
			for _, spec := range declaration.Specs {
				switch spec := spec.(type) {
				case *ast.ValueSpec:
					for _, name := range spec.Names {
						shapes = append(shapes, declarationShape{
							Kind:      kind,
							Name:      name.Name,
							Start:     offset(spec.Pos()),
							End:       offset(spec.End()),
							NameStart: offset(name.Pos()),
							NameEnd:   offset(name.End()),
						})
					}
				case *ast.TypeSpec:
					shapes = append(shapes, declarationShape{
						Kind:      "type",
						Name:      spec.Name.Name,
						Start:     offset(spec.Name.Pos()),
						End:       offset(spec.End()),
						NameStart: offset(spec.Name.Pos()),
						NameEnd:   offset(spec.Name.End()),
						Generic:   spec.TypeParams != nil,
						Alias:     spec.Assign.IsValid(),
					})
				}
			}
		}
	}
	return shapes
}

func initializerShapes(file *ast.File, offset func(token.Pos) int) []shapeComponent {
	components := []shapeComponent{}
	for _, declaration := range file.Decls {
		group, ok := declaration.(*ast.GenDecl)
		if !ok {
			continue
		}
		for _, spec := range group.Specs {
			values, ok := spec.(*ast.ValueSpec)
			if !ok {
				continue
			}
			for _, value := range values.Values {
				builder := shapeBuilder{}
				emitExpression(&builder, value, offset)
				components = append(components, builder.finish(
					offset(value.Pos()), offset(value.End()), false, false,
				))
			}
		}
	}
	sort.SliceStable(components, func(i, j int) bool {
		if components[i].Start != components[j].Start {
			return components[i].Start < components[j].Start
		}
		return components[i].End < components[j].End
	})
	return components
}

func bodyShapes(file *ast.File, offset func(token.Pos) int) []shapeComponent {
	components := []shapeComponent{}
	ast.Inspect(file, func(node ast.Node) bool {
		body, literal := (*ast.BlockStmt)(nil), false
		switch node := node.(type) {
		case *ast.FuncDecl:
			body = node.Body
		case *ast.FuncLit:
			body, literal = node.Body, true
		}
		if body == nil {
			return true
		}
		builder := shapeBuilder{}
		emitStatement(&builder, body, offset)
		components = append(components, builder.finish(
			offset(body.Pos()), offset(body.End()), literal, false,
		))
		return true
	})
	sort.SliceStable(components, func(i, j int) bool {
		if components[i].Start != components[j].Start {
			return components[i].Start < components[j].Start
		}
		return components[i].End < components[j].End
	})
	return components
}

// typeShapes mirrors the --file-bodies type enumeration, including the synthetic
// `func` prefix for interface method signatures and the skipped constraint
// unions/approximations. Nodes for method signatures are reported in the
// coordinates of that synthetic text so native parse_type output compares
// directly.
func typeShapes(file *ast.File, offset func(token.Pos) int) []shapeComponent {
	components := []shapeComponent{}
	ast.Inspect(file, func(node ast.Node) bool {
		var typeExpr ast.Expr
		switch node := node.(type) {
		case *ast.TypeSpec:
			typeExpr = node.Type
		case *ast.Field:
			typeExpr = node.Type
		case *ast.ValueSpec:
			typeExpr = node.Type
		case *ast.TypeAssertExpr:
			typeExpr = node.Type
		}
		switch term := typeExpr.(type) {
		case *ast.Ellipsis:
			typeExpr = term.Elt
		case *ast.BinaryExpr:
			typeExpr = nil // Constraint unions use a different grammar.
		case *ast.UnaryExpr:
			if term.Op == token.TILDE {
				typeExpr = nil
			}
		}
		if typeExpr == nil {
			return true
		}
		builder := shapeBuilder{}
		if function, ok := typeExpr.(*ast.FuncType); ok && !function.Func.IsValid() {
			start, end := offset(function.Pos()), offset(function.End())
			builder.add("function", shapeSpan{0, 4 + end - start})
			relative := func(position token.Pos) int { return offset(position) - start + 4 }
			emitFieldTypes(&builder, function.Params, relative)
			emitFieldTypes(&builder, function.Results, relative)
			components = append(components, builder.finish(start, end, false, true))
			return true
		}
		start := offset(typeExpr.Pos())
		relative := func(position token.Pos) int { return offset(position) - start }
		emitType(&builder, typeExpr, relative)
		components = append(components, builder.finish(start, offset(typeExpr.End()), false, false))
		return true
	})
	sort.SliceStable(components, func(i, j int) bool {
		if components[i].Start != components[j].Start {
			return components[i].Start < components[j].Start
		}
		if components[i].End != components[j].End {
			return components[i].End < components[j].End
		}
		return !components[i].Method && components[j].Method
	})
	return components
}

func emitStatement(builder *shapeBuilder, statement ast.Stmt, offset func(token.Pos) int) {
	if statement == nil {
		return
	}
	at := func(node ast.Node) shapeSpan { return shapeSpan{offset(node.Pos()), offset(node.End())} }
	switch statement := statement.(type) {
	case *ast.BlockStmt:
		builder.add("block", at(statement))
		for _, inner := range statement.List {
			emitStatement(builder, inner, offset)
		}
	case *ast.ExprStmt:
		builder.add("expr", at(statement))
		emitExpression(builder, statement.X, offset)
	case *ast.AssignStmt:
		builder.add("assign:"+statement.Tok.String(), at(statement))
		for _, expression := range statement.Lhs {
			emitExpression(builder, expression, offset)
		}
		for _, expression := range statement.Rhs {
			emitExpression(builder, expression, offset)
		}
	case *ast.IncDecStmt:
		builder.add("incdec:"+statement.Tok.String(), at(statement))
		emitExpression(builder, statement.X, offset)
	case *ast.ReturnStmt:
		builder.add("return", at(statement))
		for _, expression := range statement.Results {
			emitExpression(builder, expression, offset)
		}
	case *ast.SendStmt:
		builder.add("send", at(statement))
		emitExpression(builder, statement.Chan, offset)
		emitExpression(builder, statement.Value, offset)
	case *ast.GoStmt:
		builder.add("go", at(statement))
		emitExpression(builder, statement.Call, offset)
	case *ast.DeferStmt:
		builder.add("defer", at(statement))
		emitExpression(builder, statement.Call, offset)
	case *ast.DeclStmt:
		group, ok := statement.Decl.(*ast.GenDecl)
		if !ok {
			return
		}
		kind := "decl:" + group.Tok.String()
		grouped := group.Lparen.IsValid()
		for _, spec := range group.Specs {
			// Native ungrouped declarations span from the keyword, grouped ones
			// from the spec name.
			start := offset(spec.Pos())
			if !grouped {
				start = offset(group.TokPos)
			}
			builder.add(kind, shapeSpan{start, offset(spec.End())})
			switch spec := spec.(type) {
			case *ast.ValueSpec:
				if spec.Type != nil {
					emitType(builder, spec.Type, offset)
				}
				for _, value := range spec.Values {
					emitExpression(builder, value, offset)
				}
			case *ast.TypeSpec:
				emitType(builder, spec.Type, offset)
			}
		}
	case *ast.LabeledStmt:
		builder.add("label:"+statement.Label.Name, at(statement))
		if _, empty := statement.Stmt.(*ast.EmptyStmt); !empty {
			emitStatement(builder, statement.Stmt, offset)
		}
	case *ast.BranchStmt:
		kind := "branch:" + statement.Tok.String()
		if statement.Label != nil {
			kind += ":" + statement.Label.Name
		}
		builder.add(kind, at(statement))
	case *ast.EmptyStmt:
		return
	case *ast.IfStmt:
		builder.add("if", at(statement))
		emitStatement(builder, statement.Init, offset)
		emitExpression(builder, statement.Cond, offset)
		emitStatement(builder, statement.Body, offset)
		emitStatement(builder, statement.Else, offset)
	case *ast.ForStmt:
		builder.add("for", at(statement))
		emitStatement(builder, statement.Init, offset)
		emitExpression(builder, statement.Cond, offset)
		emitStatement(builder, statement.Post, offset)
		emitStatement(builder, statement.Body, offset)
	case *ast.RangeStmt:
		rangeToken := "illegal"
		switch statement.Tok {
		case token.ASSIGN:
			rangeToken = "="
		case token.DEFINE:
			rangeToken = ":="
		}
		builder.add("rangefor:"+rangeToken, at(statement))
		emitExpression(builder, statement.Key, offset)
		emitExpression(builder, statement.Value, offset)
		emitExpression(builder, statement.X, offset)
		emitStatement(builder, statement.Body, offset)
	case *ast.SwitchStmt:
		builder.add("switch", at(statement))
		emitStatement(builder, statement.Init, offset)
		emitExpression(builder, statement.Tag, offset)
		if statement.Body != nil {
			emitClauses(builder, statement.Body.List, statement.Body.Rbrace, offset)
		}
	case *ast.TypeSwitchStmt:
		builder.add("typeswitch", at(statement))
		emitStatement(builder, statement.Init, offset)
		emitStatement(builder, statement.Assign, offset)
		if statement.Body != nil {
			emitClauses(builder, statement.Body.List, statement.Body.Rbrace, offset)
		}
	case *ast.SelectStmt:
		builder.add("select", at(statement))
		if statement.Body != nil {
			emitClauses(builder, statement.Body.List, statement.Body.Rbrace, offset)
		}
	case *ast.BadStmt:
		return
	default:
		builder.add("unknown:"+fmt.Sprintf("%T", statement), at(statement))
	}
}

// emitClauses projects switch/select clauses. Native clause spans are the whole
// clause region (from the clause keyword to the next clause keyword or the
// closing brace) so the query scope layer retains clause locals in trailing
// trivia; go/ast's CaseClause.End is narrower, so the extent is derived here.
func emitClauses(builder *shapeBuilder, clauses []ast.Stmt, closing token.Pos, offset func(token.Pos) int) {
	for index, clause := range clauses {
		end := closing
		if index+1 < len(clauses) {
			end = clauses[index+1].Pos()
		}
		switch clause := clause.(type) {
		case *ast.CaseClause:
			builder.add("case", shapeSpan{offset(clause.Pos()), offset(end)})
			for _, expression := range clause.List {
				emitExpression(builder, expression, offset)
			}
			for _, inner := range clause.Body {
				emitStatement(builder, inner, offset)
			}
		case *ast.CommClause:
			builder.add("commcase", shapeSpan{offset(clause.Pos()), offset(end)})
			emitStatement(builder, clause.Comm, offset)
			for _, inner := range clause.Body {
				emitStatement(builder, inner, offset)
			}
		}
	}
}

func emitExpression(builder *shapeBuilder, expression ast.Expr, offset func(token.Pos) int) {
	if expression == nil {
		return
	}
	at := func(node ast.Node) shapeSpan { return shapeSpan{offset(node.Pos()), offset(node.End())} }
	switch expression := expression.(type) {
	case *ast.Ident:
		builder.add("name", at(expression))
	case *ast.BasicLit:
		builder.add("literal", at(expression))
	case *ast.FuncLit:
		builder.add("funclit", at(expression))
		// Function-literal signatures are inline type syntax inside the body.
		emitType(builder, expression.Type, offset)
	case *ast.CompositeLit:
		builder.add("composite", at(expression))
		emitExpression(builder, expression.Type, offset)
		for _, element := range expression.Elts {
			emitExpression(builder, element, offset)
		}
	case *ast.ParenExpr:
		builder.add("paren", at(expression))
		emitExpression(builder, expression.X, offset)
	case *ast.SelectorExpr:
		builder.add("selector", at(expression))
		emitExpression(builder, expression.X, offset)
	case *ast.IndexExpr:
		builder.add("index", at(expression))
		emitExpression(builder, expression.X, offset)
		emitExpression(builder, expression.Index, offset)
	case *ast.IndexListExpr:
		builder.add("index", at(expression))
		emitExpression(builder, expression.X, offset)
		for _, index := range expression.Indices {
			emitExpression(builder, index, offset)
		}
	case *ast.SliceExpr:
		builder.add("slice", at(expression))
		emitExpression(builder, expression.X, offset)
		emitExpression(builder, expression.Low, offset)
		emitExpression(builder, expression.High, offset)
		emitExpression(builder, expression.Max, offset)
	case *ast.TypeAssertExpr:
		builder.add("typeassert", at(expression))
		emitExpression(builder, expression.X, offset)
		if expression.Type != nil {
			emitType(builder, expression.Type, offset)
		}
	case *ast.CallExpr:
		kind := "call"
		if expression.Ellipsis.IsValid() {
			kind = "call..."
		}
		builder.add(kind, at(expression))
		emitExpression(builder, expression.Fun, offset)
		for _, argument := range expression.Args {
			emitExpression(builder, argument, offset)
		}
	case *ast.StarExpr:
		builder.add("unary:*", at(expression))
		emitExpression(builder, expression.X, offset)
	case *ast.UnaryExpr:
		builder.add("unary:"+expression.Op.String(), at(expression))
		emitExpression(builder, expression.X, offset)
	case *ast.BinaryExpr:
		builder.add("binary:"+expression.Op.String(), at(expression))
		emitExpression(builder, expression.X, offset)
		emitExpression(builder, expression.Y, offset)
	case *ast.KeyValueExpr:
		builder.add("keyvalue", at(expression))
		emitExpression(builder, expression.Key, offset)
		emitExpression(builder, expression.Value, offset)
	case *ast.ArrayType, *ast.MapType, *ast.ChanType, *ast.FuncType, *ast.StructType, *ast.InterfaceType:
		// Inline type syntax in expression position (conversions, composite
		// literal types) expands to the same type-shape nodes as the type section.
		emitType(builder, expression, offset)
	case *ast.Ellipsis:
		// Only reachable in type position; not a native expression node.
	case *ast.BadExpr:
		return
	default:
		builder.add("unknown:"+fmt.Sprintf("%T", expression), at(expression))
	}
}

func emitType(builder *shapeBuilder, typeExpr ast.Expr, offset func(token.Pos) int) {
	if typeExpr == nil {
		return
	}
	at := func(node ast.Node) shapeSpan { return shapeSpan{offset(node.Pos()), offset(node.End())} }
	switch typeExpr := typeExpr.(type) {
	case *ast.Ident:
		builder.add("name", at(typeExpr))
	case *ast.SelectorExpr:
		builder.add("selector", at(typeExpr))
		if _, qualified := typeExpr.X.(*ast.Ident); !qualified {
			emitType(builder, typeExpr.X, offset)
		}
	case *ast.StarExpr:
		builder.add("pointer", at(typeExpr))
		emitType(builder, typeExpr.X, offset)
	case *ast.ArrayType:
		if typeExpr.Len == nil {
			builder.add("slice", at(typeExpr))
		} else {
			builder.add("array", at(typeExpr))
		}
		// Array lengths are expressions, not type nodes; they are exercised by
		// the existing type-context gate rather than the shape differential.
		emitType(builder, typeExpr.Elt, offset)
	case *ast.MapType:
		builder.add("map", at(typeExpr))
		emitType(builder, typeExpr.Key, offset)
		emitType(builder, typeExpr.Value, offset)
	case *ast.ChanType:
		kind := "chan"
		if typeExpr.Dir == ast.SEND {
			kind = "sendchan"
		} else if typeExpr.Dir == ast.RECV {
			kind = "recvchan"
		}
		builder.add(kind, at(typeExpr))
		emitType(builder, typeExpr.Value, offset)
	case *ast.FuncType:
		builder.add("function", at(typeExpr))
		emitFieldTypes(builder, typeExpr.Params, offset)
		emitFieldTypes(builder, typeExpr.Results, offset)
	case *ast.StructType:
		builder.add("struct", at(typeExpr))
		emitFieldTypes(builder, typeExpr.Fields, offset)
	case *ast.InterfaceType:
		builder.add("interface", at(typeExpr))
		if typeExpr.Methods != nil {
			for _, field := range typeExpr.Methods.List {
				if len(field.Names) > 0 {
					if function, ok := field.Type.(*ast.FuncType); ok {
						// Native interface methods expose only their parameter
						// and result types, without a function node.
						emitFieldTypes(builder, function.Params, offset)
						emitFieldTypes(builder, function.Results, offset)
						continue
					}
				}
				emitType(builder, field.Type, offset)
			}
		}
	case *ast.ParenExpr:
		builder.add("paren", at(typeExpr))
		emitType(builder, typeExpr.X, offset)
	case *ast.IndexExpr:
		builder.add("instantiation", at(typeExpr))
		emitType(builder, typeExpr.X, offset)
		emitType(builder, typeExpr.Index, offset)
	case *ast.IndexListExpr:
		builder.add("instantiation", at(typeExpr))
		emitType(builder, typeExpr.X, offset)
		for _, index := range typeExpr.Indices {
			emitType(builder, index, offset)
		}
	case *ast.Ellipsis:
		emitType(builder, typeExpr.Elt, offset)
	case *ast.UnaryExpr:
		// Constraint approximation term: native records the inner type only.
		emitType(builder, typeExpr.X, offset)
	case *ast.BinaryExpr:
		// Constraint union: native walks both terms without a binary node.
		emitType(builder, typeExpr.X, offset)
		emitType(builder, typeExpr.Y, offset)
	default:
		builder.add("unknown:"+fmt.Sprintf("%T", typeExpr), at(typeExpr))
	}
}

func emitFieldTypes(builder *shapeBuilder, fields *ast.FieldList, offset func(token.Pos) int) {
	if fields == nil {
		return
	}
	for _, field := range fields.List {
		emitType(builder, field.Type, offset)
	}
}

// ---------------------------------------------------------------------------
// View-level shapes: imports, comments, raw strings, folding groups and folding
// nodes. Folding nodes use the native model's (kind, open, close) coordinates:
// block/composite/call use bracket offsets, case/comm use the clause colon and
// the end of the last modelled body statement. Groups encode kind plus the open
// bracket start and the close bracket end (single-byte delimiters).
// ---------------------------------------------------------------------------

func viewShapes(file *ast.File, source []byte, offset func(token.Pos) int) viewReport {
	report := viewReport{
		Imports:       []importShape{},
		CommentGroups: []commentShape{},
		CommentSpans:  []spanShape{},
		RawStrings:    []spanShape{},
	}
	for _, spec := range file.Imports {
		start := offset(spec.Path.Pos())
		name := ""
		if spec.Name != nil {
			start = offset(spec.Name.Pos())
			name = spec.Name.Name
		}
		path := ""
		if unquoted, err := strconv.Unquote(spec.Path.Value); err == nil && utf8.ValidString(unquoted) {
			// The native view only retains decoded UTF-8 import paths; invalid
			// UTF-8 values stay empty on both sides.
			path = unquoted
		}
		report.Imports = append(report.Imports, importShape{
			Start: start,
			End:   offset(spec.Path.End()),
			Name:  name,
			Path:  path,
		})
	}
	for _, group := range file.Comments {
		if len(group.List) == 0 {
			continue
		}
		report.CommentGroups = append(report.CommentGroups, commentShape{
			Group: spanShape{offset(group.Pos()), offset(group.End())},
			First: spanShape{offset(group.List[0].Pos()), offset(group.List[0].End())},
		})
		for _, comment := range group.List {
			report.CommentSpans = append(report.CommentSpans, spanShape{offset(comment.Pos()), offset(comment.End())})
		}
	}
	report.RawStrings = rawStringShapes(file, source, offset)
	groups := shapeBuilder{}
	for _, node := range viewGroupNodes(file, offset) {
		groups.add(node.Kind, shapeSpan{node.Start, node.End})
	}
	report.Groups = newShapeList(dedupGroupNodes(groups.nodes), groups.truncated)
	nodes := shapeBuilder{}
	viewFoldNodes(&nodes, file, offset)
	report.Nodes = newShapeList(dedupFoldNodes(nodes.nodes), nodes.truncated)
	return report
}

// The native view sorts and deduplicates folding nodes by (open, close, kind
// rank); the AST walk can observe the same expression through both a body arena
// and the initializer scan, so apply the same normalization here.
func dedupFoldNodes(nodes []shapeNode) []shapeNode {
	rank := func(kind string) int {
		switch kind {
		case "block":
			return 0
		case "composite":
			return 1
		case "call":
			return 2
		case "case":
			return 3
		default:
			return 4
		}
	}
	sort.Slice(nodes, func(i, j int) bool {
		if nodes[i].Start != nodes[j].Start {
			return nodes[i].Start < nodes[j].Start
		}
		if nodes[i].End != nodes[j].End {
			return nodes[i].End < nodes[j].End
		}
		return rank(nodes[i].Kind) < rank(nodes[j].Kind)
	})
	result := nodes[:0]
	for _, node := range nodes {
		if len(result) > 0 && result[len(result)-1] == node {
			continue
		}
		result = append(result, node)
	}
	return result
}

func dedupGroupNodes(nodes []shapeNode) []shapeNode {
	rank := func(kind string) int {
		switch kind {
		case "fieldlist":
			return 0
		case "decl":
			return 1
		default:
			return 2
		}
	}
	sort.Slice(nodes, func(i, j int) bool {
		if nodes[i].Start != nodes[j].Start {
			return nodes[i].Start < nodes[j].Start
		}
		if nodes[i].End != nodes[j].End {
			return nodes[i].End < nodes[j].End
		}
		return rank(nodes[i].Kind) < rank(nodes[j].Kind)
	})
	result := nodes[:0]
	for _, node := range nodes {
		if len(result) > 0 && result[len(result)-1] == node {
			continue
		}
		result = append(result, node)
	}
	return result
}

// rawStringShapes scans the original bytes for the closing back quote so CR
// removal in the decoded literal value cannot shift the span.
func rawStringShapes(file *ast.File, source []byte, offset func(token.Pos) int) []spanShape {
	spans := []spanShape{}
	ast.Inspect(file, func(node ast.Node) bool {
		literal, ok := node.(*ast.BasicLit)
		if !ok || literal.Kind != token.STRING || len(literal.Value) == 0 || literal.Value[0] != '`' {
			return true
		}
		start := offset(literal.Pos())
		end := start + 1
		for end < len(source) && source[end] != '`' {
			end++
		}
		if end < len(source) {
			end++
		}
		spans = append(spans, spanShape{start, end})
		return true
	})
	return spans
}

func viewGroupNodes(file *ast.File, offset func(token.Pos) int) []shapeNode {
	nodes := []shapeNode{}
	ast.Inspect(file, func(node ast.Node) bool {
		switch node := node.(type) {
		case *ast.FieldList:
			if node.Opening.IsValid() && node.Closing.IsValid() {
				nodes = append(nodes, shapeNode{
					Kind:  "fieldlist",
					Start: offset(node.Opening),
					End:   offset(node.Closing) + 1,
				})
			}
		case *ast.GenDecl:
			if node.Lparen.IsValid() && node.Rparen.IsValid() {
				kind := "decl"
				if node.Tok == token.IMPORT {
					kind = "imports"
				}
				nodes = append(nodes, shapeNode{
					Kind:  kind,
					Start: offset(node.Lparen),
					End:   offset(node.Rparen) + 1,
				})
			}
		}
		return true
	})
	return nodes
}

// viewFoldNodes mirrors the native view construction: every function body
// (nested literals included), every var/const initializer value in the file and
// package-level type declaration annotations (array lengths are parsed as part
// of the type expression).
func viewFoldNodes(builder *shapeBuilder, file *ast.File, offset func(token.Pos) int) {
	ast.Inspect(file, func(node ast.Node) bool {
		switch node := node.(type) {
		case *ast.FuncDecl:
			// The native view parses function/method headers through the
			// signature grammar: receiver, type parameters, parameters and
			// results all contribute array-length expressions and nested
			// function-literal bodies.
			foldFieldTypes(builder, node.Recv, offset)
			foldFieldTypes(builder, node.Type.TypeParams, offset)
			foldFieldTypes(builder, node.Type.Params, offset)
			foldFieldTypes(builder, node.Type.Results, offset)
			if node.Body != nil {
				foldStatement(builder, node.Body, offset)
			}
		case *ast.FuncLit:
			if node.Body != nil {
				foldStatement(builder, node.Body, offset)
			}
		}
		return true
	})
	ast.Inspect(file, func(node ast.Node) bool {
		if values, ok := node.(*ast.ValueSpec); ok {
			for _, value := range values.Values {
				foldExpression(builder, value, offset)
			}
		}
		return true
	})
	for _, declaration := range file.Decls {
		group, ok := declaration.(*ast.GenDecl)
		if !ok || group.Tok != token.TYPE {
			continue
		}
		for _, spec := range group.Specs {
			if typeSpec, ok := spec.(*ast.TypeSpec); ok {
				// Type-parameter constraints are parsed as part of the
				// declaration; their array lengths carry expressions.
				foldFieldTypes(builder, typeSpec.TypeParams, offset)
				foldType(builder, typeSpec.Type, offset)
			}
		}
	}
}

func foldStatement(builder *shapeBuilder, statement ast.Stmt, offset func(token.Pos) int) {
	if statement == nil {
		return
	}
	switch statement := statement.(type) {
	case *ast.BlockStmt:
		builder.add("block", shapeSpan{offset(statement.Lbrace), offset(statement.Rbrace)})
		for _, inner := range statement.List {
			foldStatement(builder, inner, offset)
		}
	case *ast.ExprStmt:
		foldExpression(builder, statement.X, offset)
	case *ast.AssignStmt:
		for _, expression := range statement.Lhs {
			foldExpression(builder, expression, offset)
		}
		for _, expression := range statement.Rhs {
			foldExpression(builder, expression, offset)
		}
	case *ast.IncDecStmt:
		foldExpression(builder, statement.X, offset)
	case *ast.ReturnStmt:
		for _, expression := range statement.Results {
			foldExpression(builder, expression, offset)
		}
	case *ast.SendStmt:
		foldExpression(builder, statement.Chan, offset)
		foldExpression(builder, statement.Value, offset)
	case *ast.GoStmt:
		foldExpression(builder, statement.Call, offset)
	case *ast.DeferStmt:
		foldExpression(builder, statement.Call, offset)
	case *ast.DeclStmt:
		group, ok := statement.Decl.(*ast.GenDecl)
		if !ok {
			return
		}
		for _, spec := range group.Specs {
			switch spec := spec.(type) {
			case *ast.ValueSpec:
				if spec.Type != nil {
					foldType(builder, spec.Type, offset)
				}
				for _, value := range spec.Values {
					foldExpression(builder, value, offset)
				}
			case *ast.TypeSpec:
				foldFieldTypes(builder, spec.TypeParams, offset)
				foldType(builder, spec.Type, offset)
			}
		}
	case *ast.LabeledStmt:
		foldStatement(builder, statement.Stmt, offset)
	case *ast.BranchStmt, *ast.EmptyStmt, *ast.BadStmt:
		return
	case *ast.IfStmt:
		foldStatement(builder, statement.Init, offset)
		foldExpression(builder, statement.Cond, offset)
		foldStatement(builder, statement.Body, offset)
		foldStatement(builder, statement.Else, offset)
	case *ast.ForStmt:
		foldStatement(builder, statement.Init, offset)
		foldExpression(builder, statement.Cond, offset)
		foldStatement(builder, statement.Post, offset)
		foldStatement(builder, statement.Body, offset)
	case *ast.RangeStmt:
		foldExpression(builder, statement.Key, offset)
		foldExpression(builder, statement.Value, offset)
		foldExpression(builder, statement.X, offset)
		foldStatement(builder, statement.Body, offset)
	case *ast.SwitchStmt:
		if statement.Body != nil {
			builder.add("block", shapeSpan{offset(statement.Body.Lbrace), offset(statement.Body.Rbrace)})
		}
		foldStatement(builder, statement.Init, offset)
		foldExpression(builder, statement.Tag, offset)
		if statement.Body != nil {
			for _, clause := range statement.Body.List {
				foldClause(builder, clause, offset)
			}
		}
	case *ast.TypeSwitchStmt:
		if statement.Body != nil {
			builder.add("block", shapeSpan{offset(statement.Body.Lbrace), offset(statement.Body.Rbrace)})
		}
		foldStatement(builder, statement.Init, offset)
		foldStatement(builder, statement.Assign, offset)
		if statement.Body != nil {
			for _, clause := range statement.Body.List {
				foldClause(builder, clause, offset)
			}
		}
	case *ast.SelectStmt:
		if statement.Body != nil {
			builder.add("block", shapeSpan{offset(statement.Body.Lbrace), offset(statement.Body.Rbrace)})
		}
		if statement.Body != nil {
			for _, clause := range statement.Body.List {
				foldClause(builder, clause, offset)
			}
		}
	}
}

func foldClause(builder *shapeBuilder, clause ast.Stmt, offset func(token.Pos) int) {
	switch clause := clause.(type) {
	case *ast.CaseClause:
		for _, expression := range clause.List {
			foldExpression(builder, expression, offset)
		}
		builder.add("case", shapeSpan{offset(clause.Colon), foldClauseEnd(clause.Colon, clause.Body, offset)})
		for _, statement := range clause.Body {
			foldStatement(builder, statement, offset)
		}
	case *ast.CommClause:
		foldStatement(builder, clause.Comm, offset)
		builder.add("comm", shapeSpan{offset(clause.Colon), foldClauseEnd(clause.Colon, clause.Body, offset)})
		for _, statement := range clause.Body {
			foldStatement(builder, statement, offset)
		}
	}
}

// foldClauseEnd matches the native clause node: the last modelled (non-empty)
// body statement end, or the colon end for an empty clause body.
func foldClauseEnd(colon token.Pos, body []ast.Stmt, offset func(token.Pos) int) int {
	for index := len(body) - 1; index >= 0; index-- {
		if _, empty := body[index].(*ast.EmptyStmt); empty {
			continue
		}
		return offset(body[index].End())
	}
	return offset(colon) + 1
}

func foldExpression(builder *shapeBuilder, expression ast.Expr, offset func(token.Pos) int) {
	if expression == nil {
		return
	}
	switch expression := expression.(type) {
	case *ast.CompositeLit:
		builder.add("composite", shapeSpan{offset(expression.Lbrace), offset(expression.Rbrace)})
		foldExpression(builder, expression.Type, offset)
		for _, element := range expression.Elts {
			foldExpression(builder, element, offset)
		}
	case *ast.CallExpr:
		builder.add("call", shapeSpan{offset(expression.Lparen), offset(expression.Rparen)})
		foldExpression(builder, expression.Fun, offset)
		for _, argument := range expression.Args {
			foldExpression(builder, argument, offset)
		}
	case *ast.ParenExpr:
		foldExpression(builder, expression.X, offset)
	case *ast.SelectorExpr:
		foldExpression(builder, expression.X, offset)
	case *ast.IndexExpr:
		foldExpression(builder, expression.X, offset)
		foldExpression(builder, expression.Index, offset)
	case *ast.IndexListExpr:
		foldExpression(builder, expression.X, offset)
		for _, index := range expression.Indices {
			foldExpression(builder, index, offset)
		}
	case *ast.SliceExpr:
		foldExpression(builder, expression.X, offset)
		foldExpression(builder, expression.Low, offset)
		foldExpression(builder, expression.High, offset)
		foldExpression(builder, expression.Max, offset)
	case *ast.UnaryExpr:
		foldExpression(builder, expression.X, offset)
	case *ast.StarExpr:
		foldExpression(builder, expression.X, offset)
	case *ast.BinaryExpr:
		foldExpression(builder, expression.X, offset)
		foldExpression(builder, expression.Y, offset)
	case *ast.KeyValueExpr:
		foldExpression(builder, expression.Key, offset)
		foldExpression(builder, expression.Value, offset)
	case *ast.TypeAssertExpr:
		foldExpression(builder, expression.X, offset)
		if expression.Type != nil {
			foldType(builder, expression.Type, offset)
		}
	case *ast.FuncLit:
		// The body is a separate body walk; only the signature type is inline.
		foldType(builder, expression.Type, offset)
	case *ast.Ident, *ast.BasicLit:
		return
	default:
		foldType(builder, expression, offset)
	}
}

// foldType walks type syntax for the expressions the native type parser turns
// into expression-arena nodes: only array lengths carry expressions.
func foldType(builder *shapeBuilder, typeExpr ast.Expr, offset func(token.Pos) int) {
	if typeExpr == nil {
		return
	}
	switch typeExpr := typeExpr.(type) {
	case *ast.ArrayType:
		if typeExpr.Len != nil {
			foldExpression(builder, typeExpr.Len, offset)
		}
		foldType(builder, typeExpr.Elt, offset)
	case *ast.MapType:
		foldType(builder, typeExpr.Key, offset)
		foldType(builder, typeExpr.Value, offset)
	case *ast.ChanType:
		foldType(builder, typeExpr.Value, offset)
	case *ast.FuncType:
		foldFieldTypes(builder, typeExpr.Params, offset)
		foldFieldTypes(builder, typeExpr.Results, offset)
	case *ast.StructType:
		foldFieldTypes(builder, typeExpr.Fields, offset)
	case *ast.InterfaceType:
		if typeExpr.Methods != nil {
			for _, field := range typeExpr.Methods.List {
				if len(field.Names) > 0 {
					if function, ok := field.Type.(*ast.FuncType); ok {
						foldFieldTypes(builder, function.Params, offset)
						foldFieldTypes(builder, function.Results, offset)
						continue
					}
				}
				foldType(builder, field.Type, offset)
			}
		}
	case *ast.ParenExpr:
		foldType(builder, typeExpr.X, offset)
	case *ast.StarExpr:
		foldType(builder, typeExpr.X, offset)
	case *ast.IndexExpr:
		foldType(builder, typeExpr.X, offset)
		foldType(builder, typeExpr.Index, offset)
	case *ast.IndexListExpr:
		foldType(builder, typeExpr.X, offset)
		for _, index := range typeExpr.Indices {
			foldType(builder, index, offset)
		}
	case *ast.Ellipsis:
		foldType(builder, typeExpr.Elt, offset)
	case *ast.UnaryExpr:
		foldType(builder, typeExpr.X, offset)
	case *ast.BinaryExpr:
		foldType(builder, typeExpr.X, offset)
		foldType(builder, typeExpr.Y, offset)
	}
}

func foldFieldTypes(builder *shapeBuilder, fields *ast.FieldList, offset func(token.Pos) int) {
	if fields == nil {
		return
	}
	for _, field := range fields.List {
		foldType(builder, field.Type, offset)
	}
}

// ---------------------------------------------------------------------------
// --negative-files / --negative-sources: bounded acceptance and recovery
// summaries for malformed syntax. Raw error messages are intentionally not
// reported; the gate compares acceptance, checks native diagnostic spans and
// keeps first/maximum error offsets as an informational recovery summary.
// ---------------------------------------------------------------------------

const maxNegativeErrors = 1000

type negativeReport struct {
	Accepted    bool `json:"accepted"`
	ErrorCount  int  `json:"error_count"`
	Capped      bool `json:"error_count_capped"`
	FirstOffset int  `json:"first_offset"`
	MaxOffset   int  `json:"max_offset"`
	Bytes       int  `json:"bytes"`
	Tokens      int  `json:"tokens"`
}

func negativeReportOf(source string, file bool) negativeReport {
	content := []byte(source)
	name := "corpus.go"
	if file {
		name = source
		read, err := os.ReadFile(source)
		if err != nil {
			panic(err)
		}
		content = read
	}
	set := token.NewFileSet()
	_, err := parser.ParseFile(set, name, content, parser.AllErrors)
	report := negativeReport{Accepted: err == nil, Bytes: len(content)}
	if file {
		// The acceptance gate already compares complete scanner token streams;
		// skip the extra full scan for corpus files.
		report.Tokens = 0
	} else {
		report.Tokens = countNegativeTokens(content)
	}
	if err == nil {
		return report
	}
	offsets := []int{}
	appendError := func(pos token.Position) {
		if len(offsets) >= maxNegativeErrors {
			return
		}
		if pos.Offset >= 0 {
			offsets = append(offsets, pos.Offset)
		}
	}
	switch errors := err.(type) {
	case scanner.ErrorList:
		for _, item := range errors {
			if len(offsets) >= maxNegativeErrors {
				break
			}
			appendError(item.Pos)
		}
		report.Capped = len(errors) > len(offsets)
	case *scanner.Error:
		appendError(errors.Pos)
	case scanner.Error:
		appendError(errors.Pos)
	}
	report.ErrorCount = len(offsets)
	if report.ErrorCount == 0 {
		// A rejection without a recoverable position still has to be compared.
		report.ErrorCount = 1
	}
	if len(offsets) == 0 {
		report.FirstOffset = -1
		report.MaxOffset = -1
		return report
	}
	report.FirstOffset = offsets[0]
	report.MaxOffset = offsets[0]
	for _, offset := range offsets[1:] {
		if offset < report.FirstOffset {
			report.FirstOffset = offset
		}
		if offset > report.MaxOffset {
			report.MaxOffset = offset
		}
	}
	return report
}

func countNegativeTokens(source []byte) int {
	var lexer scanner.Scanner
	file := token.NewFileSet().AddFile("corpus.go", -1, len(source))
	lexer.Init(file, source, func(token.Position, string) {}, 0)
	count := 0
	for {
		_, kind, _ := lexer.Scan()
		if kind == token.EOF {
			break
		}
		count++
	}
	return count
}

// ---------------------------------------------------------------------------
// --recovered-shapes: bounded recovered-structure summaries for malformed Go.
// go/parser recovers partial declarations and bodies even when errors exist; the
// native stub/block parsers must expose comparable document structure. Only
// kind/name/order and byte spans are reported, never raw AST nodes or messages.
// ---------------------------------------------------------------------------

const (
	maxRecoveredDeclarations    = 512
	maxRecoveredStatements      = 512
	maxRecoveredInitializers    = 256
	maxRecoveredExpressionNodes = 256
	maxRecoveredSourceExprNodes = 4096
	maxRecoveredNameBytes       = 256
)

type recoveredExpressionShape struct {
	Slot  int    `json:"slot"`
	Depth int    `json:"depth"`
	Kind  string `json:"kind"`
	Name  string `json:"name,omitempty"`
	Start int    `json:"start"`
	End   int    `json:"end"`
}

type recoveredStatementShape struct {
	Kind        string                     `json:"kind"`
	Start       int                        `json:"start"`
	End         int                        `json:"end"`
	Expressions []recoveredExpressionShape `json:"expressions,omitempty"`
}

type recoveredDeclarationShape struct {
	Kind       string                    `json:"kind"`
	Name       string                    `json:"name"`
	Start      int                       `json:"start"`
	End        int                       `json:"end"`
	NameStart  int                       `json:"name_start"`
	NameEnd    int                       `json:"name_end"`
	Body       bool                      `json:"body"`
	BodyStart  int                       `json:"body_start"`
	BodyEnd    int                       `json:"body_end"`
	Statements []recoveredStatementShape `json:"statements"`
}

// recoveredInitializerShape is one top-level `var`/`const` value spec; each
// value is one expression slot and nodes are pre-order with a relative depth.
type recoveredInitializerShape struct {
	Start       int                        `json:"start"`
	End         int                        `json:"end"`
	Expressions []recoveredExpressionShape `json:"expressions"`
}

type recoveredReport struct {
	Accepted        bool                        `json:"accepted"`
	ErrorCount      int                         `json:"error_count"`
	Bailed          bool                        `json:"bailed"`
	Truncated       bool                        `json:"truncated"`
	BadDeclarations []recoveredSpanShape        `json:"bad_declarations"`
	Declarations    []recoveredDeclarationShape `json:"declarations"`
	Initializers    []recoveredInitializerShape `json:"initializers"`
}

type recoveredSpanShape struct {
	Start int `json:"start"`
	End   int `json:"end"`
}

func recoveredShapesOf(source string) recoveredReport {
	report := recoveredReport{
		Accepted:        true,
		BadDeclarations: []recoveredSpanShape{},
		Declarations:    []recoveredDeclarationShape{},
		Initializers:    []recoveredInitializerShape{},
	}
	set := token.NewFileSet()
	file, err := parser.ParseFile(set, "corpus.go", source, parser.ParseComments|parser.AllErrors)
	if err != nil {
		report.Accepted = false
		report.ErrorCount = 1
		if errors, ok := err.(scanner.ErrorList); ok {
			report.ErrorCount = len(errors)
		}
	}
	offset := func(pos token.Pos) int {
		if !pos.IsValid() {
			return -1
		}
		return set.Position(pos).Offset
	}
	if file == nil {
		// go/parser bailed out after too many errors; there is no recovered tree
		// to compare against.
		report.Bailed = err != nil
		return report
	}
	expressionBudget := maxRecoveredSourceExprNodes
	for _, declaration := range file.Decls {
		if len(report.Declarations) >= maxRecoveredDeclarations {
			report.Truncated = true
			break
		}
		switch declaration := declaration.(type) {
		case *ast.BadDecl:
			// go/parser collapsed a malformed declaration; native recovery may
			// still expose a written declaration at this position.
			start, end := recoveredSpan(offset(declaration.Pos()), offset(declaration.End()))
			report.BadDeclarations = append(report.BadDeclarations, recoveredSpanShape{Start: start, End: end})
		case *ast.FuncDecl:
			kind := "func"
			if declaration.Recv != nil {
				kind = "method"
			}
			name := ""
			nameStart, nameEnd := -1, -1
			if declaration.Name != nil {
				name = truncateRecoveredName(declaration.Name.Name)
				nameStart = offset(declaration.Name.Pos())
				nameEnd = offset(declaration.Name.End())
				if !recoveredNameWritten(source, name, nameStart, nameEnd) {
					// go/parser synthesizes "_" names for missing identifiers;
					// the native view has no written token to recover.
					continue
				}
			}
			bodyStart, bodyEnd := -1, -1
			statements := []recoveredStatementShape{}
			truncatedStatements := false
			if declaration.Body != nil {
				bodyStart = offset(declaration.Body.Pos())
				bodyEnd = offset(declaration.Body.End())
				bodyStart, bodyEnd = recoveredSpan(bodyStart, bodyEnd)
				statements, truncatedStatements = recoveredBodyStatements(declaration.Body, offset, &expressionBudget)
			}
			report.Truncated = report.Truncated || truncatedStatements
			start, end := recoveredSpan(offset(declaration.Type.Func), offset(declaration.End()))
			nameStart, nameEnd = recoveredSpan(nameStart, nameEnd)
			report.Declarations = append(report.Declarations, recoveredDeclarationShape{
				Kind:       kind,
				Name:       name,
				Start:      start,
				End:        end,
				NameStart:  nameStart,
				NameEnd:    nameEnd,
				Body:       declaration.Body != nil,
				BodyStart:  bodyStart,
				BodyEnd:    bodyEnd,
				Statements: statements,
			})
		case *ast.GenDecl:
			kind := ""
			switch declaration.Tok {
			case token.TYPE:
				kind = "type"
			case token.CONST:
				kind = "const"
			case token.VAR:
				kind = "var"
			default:
				continue
			}
			for _, spec := range declaration.Specs {
				if len(report.Declarations) >= maxRecoveredDeclarations {
					report.Truncated = true
					break
				}
				switch spec := spec.(type) {
				case *ast.ValueSpec:
					start := offset(spec.Pos())
					if !declaration.Lparen.IsValid() {
						start = offset(declaration.TokPos)
					}
					end := offset(spec.End())
					start, end = recoveredSpan(start, end)
					for _, name := range spec.Names {
						if len(report.Declarations) >= maxRecoveredDeclarations {
							report.Truncated = true
							break
						}
						nameStart, nameEnd := recoveredSpan(offset(name.Pos()), offset(name.End()))
						if !recoveredNameWritten(source, truncateRecoveredName(name.Name), nameStart, nameEnd) {
							continue
						}
						report.Declarations = append(report.Declarations, recoveredDeclarationShape{
							Kind:       kind,
							Name:       truncateRecoveredName(name.Name),
							Start:      start,
							End:        end,
							NameStart:  nameStart,
							NameEnd:    nameEnd,
							BodyStart:  -1,
							BodyEnd:    -1,
							Statements: []recoveredStatementShape{},
						})
					}
				case *ast.TypeSpec:
					start := offset(spec.Name.Pos())
					if !declaration.Lparen.IsValid() {
						start = offset(declaration.TokPos)
					}
					start, end := recoveredSpan(start, offset(spec.End()))
					nameStart, nameEnd := recoveredSpan(offset(spec.Name.Pos()), offset(spec.Name.End()))
					name := truncateRecoveredName(spec.Name.Name)
					if !recoveredNameWritten(source, name, nameStart, nameEnd) {
						continue
					}
					report.Declarations = append(report.Declarations, recoveredDeclarationShape{
						Kind:       "type",
						Name:       name,
						Start:      start,
						End:        end,
						NameStart:  nameStart,
						NameEnd:    nameEnd,
						BodyStart:  -1,
						BodyEnd:    -1,
						Statements: []recoveredStatementShape{},
					})
				}
			}
		}
	}
	// Top-level var/const initializer expressions (one entry per value spec).
	for _, declaration := range file.Decls {
		group, ok := declaration.(*ast.GenDecl)
		if !ok || (group.Tok != token.CONST && group.Tok != token.VAR) {
			continue
		}
		for _, spec := range group.Specs {
			values, ok := spec.(*ast.ValueSpec)
			if !ok || len(values.Values) == 0 {
				continue
			}
			if len(report.Initializers) >= maxRecoveredInitializers {
				report.Truncated = true
				break
			}
			expressions := []recoveredExpressionShape{}
			truncated := false
			slot := 0
			for _, value := range values.Values {
				if truncated {
					break
				}
				if recoveredExpressionForest(offset, &expressions, slot, 0, value, &expressionBudget) {
					truncated = true
				}
				slot++
			}
			start, end := recoveredSpan(offset(values.Values[0].Pos()), offset(values.Values[len(values.Values)-1].End()))
			report.Initializers = append(report.Initializers, recoveredInitializerShape{
				Start:       start,
				End:         end,
				Expressions: expressions,
			})
			if truncated {
				report.Truncated = true
				break
			}
		}
		if report.Truncated {
			break
		}
	}
	return report
}

func truncateRecoveredName(name string) string {
	if len(name) <= maxRecoveredNameBytes {
		return name
	}
	cut := maxRecoveredNameBytes
	for cut > 0 && !utf8.RuneStart(name[cut]) {
		cut--
	}
	return name[:cut]
}

// recoveredSpan clamps go/ast's NoPos artifacts (recovered partial nodes can
// end before they start or lack a valid position) to a zero-width, in-source
// range so spans stay comparable and contained.
func recoveredSpan(start, end int) (int, int) {
	if start < 0 {
		start = 0
	}
	if end < start {
		end = start
	}
	return start, end
}

// recoveredNameWritten reports whether the recovered name is actually spelled by
// the source. go/parser synthesizes "_" identifiers for missing names; those have
// no native token to recover and are omitted from the comparison.
func recoveredNameWritten(source, name string, start, end int) bool {
	return start >= 0 && end >= start && end <= len(source) && source[start:end] == name
}

func recoveredBodyStatements(body *ast.BlockStmt, offset func(token.Pos) int, expressionBudget *int) ([]recoveredStatementShape, bool) {
	shapes := []recoveredStatementShape{}
	for _, statement := range body.List {
		if len(shapes) >= maxRecoveredStatements {
			return shapes, true
		}
		switch statement := statement.(type) {
		case *ast.EmptyStmt:
			continue
		case *ast.BadStmt:
			start, end := recoveredSpan(offset(statement.Pos()), offset(statement.End()))
			shapes = append(shapes, recoveredStatementShape{
				Kind:  "bad",
				Start: start,
				End:   end,
			})
		case *ast.DeclStmt:
			group, ok := statement.Decl.(*ast.GenDecl)
			if !ok {
				continue
			}
			kind := "decl:" + group.Tok.String()
			for _, spec := range group.Specs {
				if len(shapes) >= maxRecoveredStatements {
					return shapes, true
				}
				start := offset(spec.Pos())
				if !group.Lparen.IsValid() {
					start = offset(group.TokPos)
				}
				start, end := recoveredSpan(start, offset(spec.End()))
				expressions := []recoveredExpressionShape{}
				truncated := false
				if values, ok := spec.(*ast.ValueSpec); ok {
					slot := 0
					for _, value := range values.Values {
						if truncated {
							break
						}
						if recoveredExpressionForest(offset, &expressions, slot, 0, value, expressionBudget) {
							truncated = true
						}
						slot++
					}
				}
				shapes = append(shapes, recoveredStatementShape{
					Kind:        kind,
					Start:       start,
					End:         end,
					Expressions: expressions,
				})
				if truncated {
					return shapes, true
				}
			}
		default:
			kind, ok := recoveredStatementKind(statement)
			if !ok {
				continue
			}
			start, end := recoveredSpan(offset(statement.Pos()), offset(statement.End()))
			expressions, truncated := recoveredStatementExpressions(statement, offset, expressionBudget)
			shapes = append(shapes, recoveredStatementShape{
				Kind:        kind,
				Start:       start,
				End:         end,
				Expressions: expressions,
			})
			if truncated {
				return shapes, true
			}
		}
	}
	return shapes, false
}

// recoveredStatementExpressions projects expression roots attached to one direct
// statement: assignment sides, return values, condition/tag/value expressions and
// local declaration initializers. Nested bodies are not descended (they are
// separate direct-body scopes). Each root is one slot; nodes are pre-order.
func recoveredStatementExpressions(statement ast.Stmt, offset func(token.Pos) int, budget *int) ([]recoveredExpressionShape, bool) {
	expressions := []recoveredExpressionShape{}
	slot := 0
	truncated := false
	appendExpression := func(expression ast.Expr) {
		if expression == nil || truncated {
			return
		}
		if recoveredExpressionForest(offset, &expressions, slot, 0, expression, budget) {
			truncated = true
		}
		slot++
	}
	appendSimple := func(statement ast.Stmt) {
		if statement == nil || truncated {
			return
		}
		switch statement := statement.(type) {
		case *ast.ExprStmt:
			appendExpression(statement.X)
		case *ast.AssignStmt:
			for _, expression := range statement.Lhs {
				appendExpression(expression)
			}
			for _, expression := range statement.Rhs {
				appendExpression(expression)
			}
		case *ast.SendStmt:
			appendExpression(statement.Chan)
			appendExpression(statement.Value)
		case *ast.IncDecStmt:
			appendExpression(statement.X)
		}
	}
	switch statement := statement.(type) {
	case *ast.ExprStmt:
		appendExpression(statement.X)
	case *ast.AssignStmt:
		for _, expression := range statement.Lhs {
			appendExpression(expression)
		}
		for _, expression := range statement.Rhs {
			appendExpression(expression)
		}
	case *ast.ReturnStmt:
		for _, expression := range statement.Results {
			appendExpression(expression)
		}
	case *ast.SendStmt:
		appendExpression(statement.Chan)
		appendExpression(statement.Value)
	case *ast.IncDecStmt:
		appendExpression(statement.X)
	case *ast.GoStmt:
		appendExpression(statement.Call)
	case *ast.DeferStmt:
		appendExpression(statement.Call)
	case *ast.DeclStmt:
		if group, ok := statement.Decl.(*ast.GenDecl); ok {
			for _, spec := range group.Specs {
				if values, ok := spec.(*ast.ValueSpec); ok {
					for _, expression := range values.Values {
						appendExpression(expression)
					}
				}
			}
		}
	case *ast.IfStmt:
		appendSimple(statement.Init)
		appendExpression(statement.Cond)
	case *ast.ForStmt:
		appendSimple(statement.Init)
		appendExpression(statement.Cond)
		appendSimple(statement.Post)
	case *ast.RangeStmt:
		appendExpression(statement.X)
		appendExpression(statement.Key)
		appendExpression(statement.Value)
	case *ast.SwitchStmt:
		appendSimple(statement.Init)
		appendExpression(statement.Tag)
	case *ast.TypeSwitchStmt:
		appendSimple(statement.Init)
		appendSimple(statement.Assign)
	}
	return expressions, truncated
}

// recoveredExpressionForest appends one expression root and its descendants in
// pre-order with a relative depth. Function-literal bodies/signatures and type
// syntax internals remain opaque leaves on both sides.
func recoveredExpressionForest(offset func(token.Pos) int, builder *[]recoveredExpressionShape, slot, depth int, expression ast.Expr, budget *int) bool {
	if expression == nil {
		return false
	}
	if *budget <= 0 {
		return true
	}
	kind, name, children := recoveredExpressionShapeOf(expression)
	start, end := recoveredSpan(offset(expression.Pos()), offset(expression.End()))
	*builder = append(*builder, recoveredExpressionShape{
		Slot:  slot,
		Depth: depth,
		Kind:  kind,
		Name:  name,
		Start: start,
		End:   end,
	})
	*budget--
	for _, child := range children {
		if recoveredExpressionForest(offset, builder, slot, depth+1, child, budget) {
			return true
		}
	}
	return false
}

func recoveredExpressionShapeOf(expression ast.Expr) (string, string, []ast.Expr) {
	switch expression := expression.(type) {
	case *ast.Ident:
		return "name", truncateRecoveredName(expression.Name), nil
	case *ast.BasicLit:
		return "lit", "", nil
	case *ast.BadExpr:
		return "bad", "", nil
	case *ast.ArrayType, *ast.MapType, *ast.ChanType, *ast.FuncType, *ast.StructType, *ast.InterfaceType:
		return "type", "", nil
	case *ast.CompositeLit:
		children := []ast.Expr{}
		if expression.Type != nil {
			children = append(children, expression.Type)
		}
		return "composite", "", append(children, expression.Elts...)
	case *ast.KeyValueExpr:
		return "keyvalue", "", []ast.Expr{expression.Key, expression.Value}
	case *ast.FuncLit:
		return "funclit", "", nil
	case *ast.UnaryExpr:
		if expression.Op == token.ARROW {
			return "receive", "", []ast.Expr{expression.X}
		}
		return "unary:" + expression.Op.String(), "", []ast.Expr{expression.X}
	case *ast.StarExpr:
		return "star", "", []ast.Expr{expression.X}
	case *ast.BinaryExpr:
		return "binary:" + expression.Op.String(), "", []ast.Expr{expression.X, expression.Y}
	case *ast.ParenExpr:
		return "paren", "", []ast.Expr{expression.X}
	case *ast.SelectorExpr:
		return "selector", truncateRecoveredName(expression.Sel.Name), []ast.Expr{expression.X}
	case *ast.CallExpr:
		children := append([]ast.Expr{expression.Fun}, expression.Args...)
		if expression.Ellipsis.IsValid() {
			return "call:...", "", children
		}
		return "call", "", children
	case *ast.IndexExpr:
		return "index", "", []ast.Expr{expression.X, expression.Index}
	case *ast.IndexListExpr:
		// The native expression parser cannot distinguish an index from a
		// generic instantiation in syntax alone; both index forms canonicalize to
		// one kind and the operand count carries the arity.
		return "index", "", append([]ast.Expr{expression.X}, expression.Indices...)
	case *ast.SliceExpr:
		children := []ast.Expr{expression.X}
		for _, expression := range []ast.Expr{expression.Low, expression.High, expression.Max} {
			if expression != nil {
				children = append(children, expression)
			}
		}
		return "slice", "", children
	case *ast.TypeAssertExpr:
		return "typeassert", "", []ast.Expr{expression.X}
	case *ast.Ellipsis:
		if expression.Elt != nil {
			return "ellipsis", "", []ast.Expr{expression.Elt}
		}
		return "ellipsis", "", nil
	}
	return "unknown", "", nil
}

func recoveredStatementKind(statement ast.Stmt) (string, bool) {
	switch statement.(type) {
	case *ast.BlockStmt:
		return "block", true
	case *ast.ExprStmt:
		return "expr", true
	case *ast.AssignStmt:
		return "assign", true
	case *ast.IncDecStmt:
		return "incdec", true
	case *ast.ReturnStmt:
		return "return", true
	case *ast.SendStmt:
		return "send", true
	case *ast.GoStmt:
		return "go", true
	case *ast.DeferStmt:
		return "defer", true
	case *ast.LabeledStmt:
		return "label", true
	case *ast.BranchStmt:
		return "branch", true
	case *ast.IfStmt:
		return "if", true
	case *ast.ForStmt:
		return "for", true
	case *ast.RangeStmt:
		return "range", true
	case *ast.SwitchStmt:
		return "switch", true
	case *ast.TypeSwitchStmt:
		return "typeswitch", true
	case *ast.SelectStmt:
		return "select", true
	case *ast.DeclStmt:
		return "decl", true
	}
	return "unknown", true
}
