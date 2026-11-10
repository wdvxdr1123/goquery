// Syntax acceptance oracle; no AST is serialized or compared.
package main

import (
	"bytes"
	"encoding/json"
	"go/ast"
	"go/build"
	"go/build/constraint"
	"go/parser"
	"go/scanner"
	"go/token"
	"io"
	"os"
	"strings"
	"unicode"
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
