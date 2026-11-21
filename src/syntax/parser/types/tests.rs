use crate::syntax::ast::expression::ExprKind;
use crate::syntax::ast::types::{
    ChanDir, InterfaceMemberNode, MAX_TYPE_NODES, TypeNodeId, TypeNodeKind, TypeSyntax,
    TypeTreeKind,
};
use crate::syntax::parser::types::{parse_constraint, parse_type, parse_type_prefix};
use std::ops::Range;

fn slice<'a>(source: &'a str, span: &Range<u32>) -> &'a str {
    &source[span.start as usize..span.end as usize]
}

/// Recursively verifies every reachable ID, slice range, byte span, and the
/// children-first (acyclic) invariant.
fn assert_tree_well_formed(source: &str, parsed: &TypeSyntax) {
    let Some(root) = parsed.root() else {
        assert!(parsed.tree_overflowed());
        assert!(parsed.tree_nodes().is_empty());
        assert!(parsed.field_nodes().is_empty());
        assert!(parsed.interface_member_nodes().is_empty());
        assert!(parsed.sig_nodes().is_empty());
        assert!(parsed.param_nodes().is_empty());
        assert!(parsed.type_node_lists().is_empty());
        return;
    };
    assert!(!parsed.tree_overflowed());
    verify_node(source, parsed, root);
}

fn verify_node(source: &str, parsed: &TypeSyntax, id: TypeNodeId) {
    let node = parsed.tree_node(id).expect("valid TypeNodeId");
    assert!(node.span.start <= node.span.end);
    assert!(node.span.end as usize <= source.len());
    match &node.kind {
        TypeTreeKind::Missing => {}
        TypeTreeKind::Parenthesized(inner)
        | TypeTreeKind::Pointer(inner)
        | TypeTreeKind::Slice(inner)
        | TypeTreeKind::Approx(inner) => {
            assert!(inner.raw() < id.raw());
            verify_node(source, parsed, *inner);
        }
        TypeTreeKind::Array { len_expr, elem } => {
            if let Some(expr_id) = len_expr {
                assert!(parsed.expression(*expr_id).is_some());
            }
            assert!(elem.raw() < id.raw());
            verify_node(source, parsed, *elem);
        }
        TypeTreeKind::Map { key, value } => {
            assert!(key.raw() < id.raw());
            assert!(value.raw() < id.raw());
            verify_node(source, parsed, *key);
            verify_node(source, parsed, *value);
        }
        TypeTreeKind::Chan { elem, .. } => {
            assert!(elem.raw() < id.raw());
            verify_node(source, parsed, *elem);
        }
        TypeTreeKind::Func(sig_id) => {
            verify_sig(source, parsed, *sig_id, id);
        }
        TypeTreeKind::Struct { fields } => {
            assert!(fields.start <= fields.end);
            assert!(fields.end as usize <= parsed.field_nodes().len());
            for field in parsed.struct_fields(fields.clone()) {
                if let Some(name) = &field.name {
                    assert!(name.start <= name.end && name.end as usize <= source.len());
                }
                if let Some(tag) = &field.tag {
                    assert!(tag.start <= tag.end && tag.end as usize <= source.len());
                }
                assert!(field.ty.raw() < id.raw());
                verify_node(source, parsed, field.ty);
            }
        }
        TypeTreeKind::Interface { members } => {
            assert!(members.start <= members.end);
            assert!(members.end as usize <= parsed.interface_member_nodes().len());
            for member in parsed.interface_members(members.clone()) {
                match member {
                    InterfaceMemberNode::Method { name, sig } => {
                        assert!(name.start <= name.end && name.end as usize <= source.len());
                        verify_sig(source, parsed, *sig, id);
                    }
                    InterfaceMemberNode::Embedded(ty) => {
                        assert!(ty.raw() < id.raw());
                        verify_node(source, parsed, *ty);
                    }
                }
            }
        }
        TypeTreeKind::Union { terms } => {
            assert!(terms.start <= terms.end);
            for &term in parsed.type_node_list(terms.clone()) {
                assert!(term.raw() < id.raw());
                verify_node(source, parsed, term);
            }
        }
        TypeTreeKind::Name { span } => {
            assert!(span.start <= span.end && span.end as usize <= source.len());
        }
        TypeTreeKind::Qualified { qual, name } => {
            assert!(qual.start <= qual.end && qual.end as usize <= source.len());
            assert!(name.start <= name.end && name.end as usize <= source.len());
        }
        TypeTreeKind::Instantiation { base, args } => {
            assert!(base.raw() < id.raw());
            verify_node(source, parsed, *base);
            assert!(args.start <= args.end);
            for &arg in parsed.type_node_list(args.clone()) {
                assert!(arg.raw() < id.raw());
                verify_node(source, parsed, arg);
            }
        }
    }
}

fn verify_sig(
    source: &str,
    parsed: &TypeSyntax,
    sig_id: crate::syntax::SigNodeId,
    owner: TypeNodeId,
) {
    let sig = parsed.sig_node(sig_id).expect("valid SigNodeId");
    assert!(sig.span.start <= sig.span.end && sig.span.end as usize <= source.len());
    assert!(sig.params.start <= sig.params.end);
    assert!(sig.params.end as usize <= parsed.param_nodes().len());
    assert!(sig.results.start <= sig.results.end);
    assert!(sig.results.end as usize <= parsed.param_nodes().len());
    for param in parsed
        .sig_params(sig.params.clone())
        .iter()
        .chain(parsed.sig_params(sig.results.clone()))
    {
        if let Some(name) = &param.name {
            assert!(name.start <= name.end && name.end as usize <= source.len());
        }
        assert!(param.ty.raw() < owner.raw());
        verify_node(source, parsed, param.ty);
    }
}

#[test]
fn basic_compound_types() {
    let source = "(*map[string]chan<- <-chan chan []pkg.Elem)";
    let parsed = parse_type(source);
    assert!(parsed.diagnostics().is_empty());
    assert_tree_well_formed(source, &parsed);

    let root = parsed.root().unwrap();
    let TypeTreeKind::Parenthesized(ptr_id) = parsed.tree_node(root).unwrap().kind else {
        panic!("expected Parenthesized");
    };
    let TypeTreeKind::Pointer(map_id) = parsed.tree_node(ptr_id).unwrap().kind else {
        panic!("expected Pointer");
    };
    let TypeTreeKind::Map { key, value } = parsed.tree_node(map_id).unwrap().kind else {
        panic!("expected Map");
    };
    let TypeTreeKind::Name { span } = &parsed.tree_node(key).unwrap().kind else {
        panic!("expected Name key");
    };
    assert_eq!(slice(source, span), "string");

    let TypeTreeKind::Chan {
        dir: ChanDir::Send,
        elem: recv_id,
    } = parsed.tree_node(value).unwrap().kind
    else {
        panic!("expected Send chan");
    };
    let TypeTreeKind::Chan {
        dir: ChanDir::Recv,
        elem: both_id,
    } = parsed.tree_node(recv_id).unwrap().kind
    else {
        panic!("expected Recv chan");
    };
    let TypeTreeKind::Chan {
        dir: ChanDir::Both,
        elem: slice_id,
    } = parsed.tree_node(both_id).unwrap().kind
    else {
        panic!("expected Both chan");
    };
    let TypeTreeKind::Slice(elem_id) = parsed.tree_node(slice_id).unwrap().kind else {
        panic!("expected Slice");
    };
    let TypeTreeKind::Qualified { qual, name } = &parsed.tree_node(elem_id).unwrap().kind else {
        panic!("expected Qualified");
    };
    assert_eq!(slice(source, qual), "pkg");
    assert_eq!(slice(source, name), "Elem");
}

#[test]
fn array_length_expressions_and_inferred_arrays() {
    let source = "[N+1][2*M]T";
    let parsed = parse_type(source);
    assert!(parsed.diagnostics().is_empty());
    assert_tree_well_formed(source, &parsed);

    let root = parsed.root().unwrap();
    let TypeTreeKind::Array {
        len_expr: Some(outer_len),
        elem: inner_id,
    } = parsed.tree_node(root).unwrap().kind
    else {
        panic!("expected outer Array with len_expr");
    };
    let outer_expr = parsed.expression(outer_len).unwrap();
    assert!(matches!(outer_expr.kind, ExprKind::Binary { .. }));
    assert_eq!(&source[outer_expr.span.clone()], "N+1");

    let TypeTreeKind::Array {
        len_expr: Some(inner_len),
        elem: elem_id,
    } = parsed.tree_node(inner_id).unwrap().kind
    else {
        panic!("expected inner Array with len_expr");
    };
    let inner_expr = parsed.expression(inner_len).unwrap();
    assert!(matches!(inner_expr.kind, ExprKind::Binary { .. }));
    assert_eq!(&source[inner_expr.span.clone()], "2*M");

    let TypeTreeKind::Name { span } = &parsed.tree_node(elem_id).unwrap().kind else {
        panic!("expected Name elem");
    };
    assert_eq!(slice(source, span), "T");

    // Inferred array length `[...]byte` stores `len_expr: None`.
    let inferred_src = "[...]byte";
    let inferred = parse_type(inferred_src);
    assert!(inferred.diagnostics().is_empty());
    assert_tree_well_formed(inferred_src, &inferred);
    let TypeTreeKind::Array {
        len_expr: None,
        elem,
    } = inferred.tree_node(inferred.root().unwrap()).unwrap().kind
    else {
        panic!("expected inferred Array");
    };
    let TypeTreeKind::Name { span } = &inferred.tree_node(elem).unwrap().kind else {
        panic!("expected Name");
    };
    assert_eq!(slice(inferred_src, span), "byte");
}

#[test]
fn generic_instantiations() {
    let source = "pkg.M[K, []V, Box[int]]";
    let parsed = parse_type(source);
    assert!(parsed.diagnostics().is_empty());
    assert_tree_well_formed(source, &parsed);

    let root = parsed.root().unwrap();
    let TypeTreeKind::Instantiation { base, args } = parsed.tree_node(root).unwrap().kind.clone()
    else {
        panic!("expected Instantiation");
    };
    let TypeTreeKind::Qualified { qual, name } = &parsed.tree_node(base).unwrap().kind else {
        panic!("expected Qualified base");
    };
    assert_eq!(slice(source, qual), "pkg");
    assert_eq!(slice(source, name), "M");

    let arg_ids = parsed.type_node_list(args);
    assert_eq!(arg_ids.len(), 3);
    assert!(matches!(
        &parsed.tree_node(arg_ids[0]).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "K"
    ));
    let TypeTreeKind::Slice(v_id) = parsed.tree_node(arg_ids[1]).unwrap().kind else {
        panic!("expected []V");
    };
    assert!(matches!(
        &parsed.tree_node(v_id).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "V"
    ));
    let TypeTreeKind::Instantiation {
        base: box_base,
        args: box_args,
    } = parsed.tree_node(arg_ids[2]).unwrap().kind.clone()
    else {
        panic!("expected nested Box[int]");
    };
    assert!(matches!(
        &parsed.tree_node(box_base).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "Box"
    ));
    let box_arg_ids = parsed.type_node_list(box_args);
    assert_eq!(box_arg_ids.len(), 1);
    assert!(matches!(
        &parsed.tree_node(box_arg_ids[0]).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "int"
    ));
}

#[test]
fn nested_structs_grouped_and_embedded_fields() {
    let source = r#"struct {
        a, b int `json:"ab"`
        inner struct {
            x string
            *io.Reader
        }
        Base
        *pkg.Gen[K, []V] `tag:"g"`
        tail bool
    }"#;
    let parsed = parse_type(source);
    assert!(
        parsed.diagnostics().is_empty(),
        "{:?}",
        parsed.diagnostics()
    );
    assert_tree_well_formed(source, &parsed);

    let root = parsed.root().unwrap();
    let TypeTreeKind::Struct { fields } = parsed.tree_node(root).unwrap().kind.clone() else {
        panic!("expected outer Struct");
    };
    let outer_fields = parsed.struct_fields(fields);
    assert_eq!(outer_fields.len(), 6);

    // Grouped `a, b int` share the exact same TypeNodeId and tag.
    assert_eq!(slice(source, outer_fields[0].name.as_ref().unwrap()), "a");
    assert_eq!(slice(source, outer_fields[1].name.as_ref().unwrap()), "b");
    assert_eq!(outer_fields[0].ty, outer_fields[1].ty);
    assert!(!outer_fields[0].embedded);
    assert!(!outer_fields[1].embedded);
    assert_eq!(
        slice(source, outer_fields[0].tag.as_ref().unwrap()),
        "`json:\"ab\"`"
    );
    assert_eq!(outer_fields[0].tag, outer_fields[1].tag);

    // Nested struct fields are contiguous and not interleaved with outer fields.
    assert_eq!(
        slice(source, outer_fields[2].name.as_ref().unwrap()),
        "inner"
    );
    assert!(!outer_fields[2].embedded);
    let TypeTreeKind::Struct {
        fields: inner_range,
    } = parsed.tree_node(outer_fields[2].ty).unwrap().kind.clone()
    else {
        panic!("expected nested Struct");
    };
    let inner_fields = parsed.struct_fields(inner_range);
    assert_eq!(inner_fields.len(), 2);
    assert_eq!(slice(source, inner_fields[0].name.as_ref().unwrap()), "x");
    assert!(!inner_fields[0].embedded);
    assert_eq!(
        slice(source, inner_fields[1].name.as_ref().unwrap()),
        "Reader"
    );
    assert!(inner_fields[1].embedded);
    assert!(matches!(
        parsed.tree_node(inner_fields[1].ty).unwrap().kind,
        TypeTreeKind::Pointer(_)
    ));

    // Bare embedded `Base`.
    assert_eq!(
        slice(source, outer_fields[3].name.as_ref().unwrap()),
        "Base"
    );
    assert!(outer_fields[3].embedded);
    assert!(matches!(
        &parsed.tree_node(outer_fields[3].ty).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "Base"
    ));

    // Pointer-qualified-generic embedded `*pkg.Gen[K, []V]`.
    assert_eq!(slice(source, outer_fields[4].name.as_ref().unwrap()), "Gen");
    assert!(outer_fields[4].embedded);
    assert_eq!(
        slice(source, outer_fields[4].tag.as_ref().unwrap()),
        "`tag:\"g\"`"
    );
    let TypeTreeKind::Pointer(gen_inst) = parsed.tree_node(outer_fields[4].ty).unwrap().kind else {
        panic!("expected Pointer to Instantiation");
    };
    assert!(matches!(
        parsed.tree_node(gen_inst).unwrap().kind,
        TypeTreeKind::Instantiation { .. }
    ));

    // Trailing field `tail bool`.
    assert_eq!(
        slice(source, outer_fields[5].name.as_ref().unwrap()),
        "tail"
    );
    assert!(!outer_fields[5].embedded);
}

#[test]
fn interfaces_with_methods_embeds_unions_and_approx() {
    let source = r#"interface {
        io.Closer
        Read(p []byte) (n int, err error)
        Log(fmt string, args ...any)
        ~int | ~string | float64
        ~[]byte
        interface { Ping() bool }
    }"#;
    let parsed = parse_type(source);
    assert!(
        parsed.diagnostics().is_empty(),
        "{:?}",
        parsed.diagnostics()
    );
    assert_tree_well_formed(source, &parsed);

    let root = parsed.root().unwrap();
    let TypeTreeKind::Interface { members } = parsed.tree_node(root).unwrap().kind.clone() else {
        panic!("expected Interface");
    };
    let members = parsed.interface_members(members);
    assert_eq!(members.len(), 6);

    // 0: embedded io.Closer
    let InterfaceMemberNode::Embedded(closer_id) = members[0] else {
        panic!("expected Embedded io.Closer");
    };
    assert!(matches!(
        &parsed.tree_node(closer_id).unwrap().kind,
        TypeTreeKind::Qualified { qual, name }
            if slice(source, qual) == "io" && slice(source, name) == "Closer"
    ));

    // 1: method Read(p []byte) (n int, err error)
    let InterfaceMemberNode::Method { name, sig } = &members[1] else {
        panic!("expected Method Read");
    };
    assert_eq!(slice(source, name), "Read");
    let read_sig = parsed.sig_node(*sig).unwrap();
    assert!(!read_sig.variadic);
    let read_params = parsed.sig_params(read_sig.params.clone());
    assert_eq!(read_params.len(), 1);
    assert_eq!(slice(source, read_params[0].name.as_ref().unwrap()), "p");
    assert!(matches!(
        parsed.tree_node(read_params[0].ty).unwrap().kind,
        TypeTreeKind::Slice(_)
    ));
    let read_results = parsed.sig_params(read_sig.results.clone());
    assert_eq!(read_results.len(), 2);
    assert_eq!(slice(source, read_results[0].name.as_ref().unwrap()), "n");
    assert_eq!(slice(source, read_results[1].name.as_ref().unwrap()), "err");

    // 2: variadic method Log(fmt string, args ...any)
    let InterfaceMemberNode::Method { name, sig } = &members[2] else {
        panic!("expected Method Log");
    };
    assert_eq!(slice(source, name), "Log");
    let log_sig = parsed.sig_node(*sig).unwrap();
    assert!(log_sig.variadic);
    let log_params = parsed.sig_params(log_sig.params.clone());
    assert_eq!(log_params.len(), 2);
    assert!(!log_params[0].variadic);
    assert!(log_params[1].variadic);
    assert_eq!(slice(source, log_params[1].name.as_ref().unwrap()), "args");
    assert!(matches!(
        &parsed.tree_node(log_params[1].ty).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "any"
    ));
    assert!(parsed.sig_params(log_sig.results.clone()).is_empty());

    // 3: union `~int | ~string | float64`
    let InterfaceMemberNode::Embedded(union_id) = members[3] else {
        panic!("expected Embedded Union");
    };
    let TypeTreeKind::Union { terms } = parsed.tree_node(union_id).unwrap().kind.clone() else {
        panic!("expected Union");
    };
    let term_ids = parsed.type_node_list(terms);
    assert_eq!(term_ids.len(), 3);
    let TypeTreeKind::Approx(t0) = parsed.tree_node(term_ids[0]).unwrap().kind else {
        panic!("expected Approx ~int");
    };
    assert!(matches!(
        &parsed.tree_node(t0).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "int"
    ));
    let TypeTreeKind::Approx(t1) = parsed.tree_node(term_ids[1]).unwrap().kind else {
        panic!("expected Approx ~string");
    };
    assert!(matches!(
        &parsed.tree_node(t1).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "string"
    ));
    assert!(matches!(
        &parsed.tree_node(term_ids[2]).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "float64"
    ));

    // 4: single approx `~[]byte` (not wrapped in Union)
    let InterfaceMemberNode::Embedded(approx_id) = members[4] else {
        panic!("expected Embedded Approx");
    };
    let TypeTreeKind::Approx(slice_byte) = parsed.tree_node(approx_id).unwrap().kind else {
        panic!("expected Approx(~[]byte)");
    };
    assert!(matches!(
        parsed.tree_node(slice_byte).unwrap().kind,
        TypeTreeKind::Slice(_)
    ));

    // 5: nested embedded interface
    let InterfaceMemberNode::Embedded(nested_iface) = members[5] else {
        panic!("expected Embedded nested interface");
    };
    let TypeTreeKind::Interface {
        members: nested_members,
    } = parsed.tree_node(nested_iface).unwrap().kind.clone()
    else {
        panic!("expected nested Interface");
    };
    let nested = parsed.interface_members(nested_members);
    assert_eq!(nested.len(), 1);
    assert!(matches!(
        &nested[0],
        InterfaceMemberNode::Method { name, .. } if slice(source, name) == "Ping"
    ));
}

#[test]
fn parse_constraint_direct_union_and_approx() {
    let source = "~T | pkg.U[int]";
    let parsed = parse_constraint(source);
    assert!(parsed.diagnostics().is_empty());
    assert_tree_well_formed(source, &parsed);

    let root = parsed.root().unwrap();
    let TypeTreeKind::Union { terms } = parsed.tree_node(root).unwrap().kind.clone() else {
        panic!("expected Union");
    };
    let term_ids = parsed.type_node_list(terms);
    assert_eq!(term_ids.len(), 2);
    let TypeTreeKind::Approx(t_id) = parsed.tree_node(term_ids[0]).unwrap().kind else {
        panic!("expected Approx");
    };
    assert!(matches!(
        &parsed.tree_node(t_id).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "T"
    ));
    assert!(matches!(
        parsed.tree_node(term_ids[1]).unwrap().kind,
        TypeTreeKind::Instantiation { .. }
    ));

    // Single approx constraint
    let single = parse_constraint("~string");
    assert!(single.diagnostics().is_empty());
    assert_tree_well_formed("~string", &single);
    assert!(matches!(
        single.tree_node(single.root().unwrap()).unwrap().kind,
        TypeTreeKind::Approx(_)
    ));
}

#[test]
fn function_types_with_variadics_named_results_and_nesting() {
    let source = "func(a, b int, cb func(int) (x, y string), rest ...[]byte) (ok bool, err error)";
    let parsed = parse_type(source);
    assert!(
        parsed.diagnostics().is_empty(),
        "{:?}",
        parsed.diagnostics()
    );
    assert_tree_well_formed(source, &parsed);

    let root = parsed.root().unwrap();
    let TypeTreeKind::Func(outer_sig_id) = parsed.tree_node(root).unwrap().kind else {
        panic!("expected Func");
    };
    let outer_sig = parsed.sig_node(outer_sig_id).unwrap();
    assert!(outer_sig.variadic);
    assert_eq!(
        slice(source, &outer_sig.span),
        "(a, b int, cb func(int) (x, y string), rest ...[]byte) (ok bool, err error)"
    );

    let params = parsed.sig_params(outer_sig.params.clone());
    assert_eq!(params.len(), 4);
    assert_eq!(slice(source, params[0].name.as_ref().unwrap()), "a");
    assert_eq!(slice(source, params[1].name.as_ref().unwrap()), "b");
    assert_eq!(params[0].ty, params[1].ty);
    assert!(!params[0].variadic);
    assert!(!params[1].variadic);

    // Nested callback signature is isolated and not interleaved.
    assert_eq!(slice(source, params[2].name.as_ref().unwrap()), "cb");
    let TypeTreeKind::Func(cb_sig_id) = parsed.tree_node(params[2].ty).unwrap().kind else {
        panic!("expected nested Func");
    };
    let cb_sig = parsed.sig_node(cb_sig_id).unwrap();
    assert!(!cb_sig.variadic);
    let cb_params = parsed.sig_params(cb_sig.params.clone());
    assert_eq!(cb_params.len(), 1);
    assert!(cb_params[0].name.is_none());
    assert!(matches!(
        &parsed.tree_node(cb_params[0].ty).unwrap().kind,
        TypeTreeKind::Name { span } if slice(source, span) == "int"
    ));
    let cb_results = parsed.sig_params(cb_sig.results.clone());
    assert_eq!(cb_results.len(), 2);
    assert_eq!(slice(source, cb_results[0].name.as_ref().unwrap()), "x");
    assert_eq!(slice(source, cb_results[1].name.as_ref().unwrap()), "y");
    assert_eq!(cb_results[0].ty, cb_results[1].ty);

    // Variadic param stores element type `[]byte`, not `...[]byte`.
    assert_eq!(slice(source, params[3].name.as_ref().unwrap()), "rest");
    assert!(params[3].variadic);
    assert!(matches!(
        parsed.tree_node(params[3].ty).unwrap().kind,
        TypeTreeKind::Slice(_)
    ));

    let results = parsed.sig_params(outer_sig.results.clone());
    assert_eq!(results.len(), 2);
    assert_eq!(slice(source, results[0].name.as_ref().unwrap()), "ok");
    assert_eq!(slice(source, results[1].name.as_ref().unwrap()), "err");

    // Unnamed variadic + single unparenthesized result
    let short_src = "func(...int) error";
    let short = parse_type(short_src);
    assert!(short.diagnostics().is_empty());
    assert_tree_well_formed(short_src, &short);
    let TypeTreeKind::Func(sig_id) = short.tree_node(short.root().unwrap()).unwrap().kind else {
        panic!("expected Func");
    };
    let sig = short.sig_node(sig_id).unwrap();
    assert!(sig.variadic);
    let p = short.sig_params(sig.params.clone());
    assert_eq!(p.len(), 1);
    assert!(p[0].name.is_none());
    assert!(p[0].variadic);
    let r = short.sig_params(sig.results.clone());
    assert_eq!(r.len(), 1);
    assert!(r[0].name.is_none());
}

#[test]
fn parse_type_prefix_and_legacy_backwards_compatibility() {
    let source = "map[string][]int = nil";
    let parsed = parse_type_prefix(source);
    assert!(parsed.diagnostics().is_empty());
    assert_eq!(&source[parsed.span.clone()], "map[string][]int");
    assert_tree_well_formed(source, &parsed);

    // Legacy `type_nodes` remains identical in pre-order.
    let legacy_kinds: Vec<TypeNodeKind> = parsed.type_nodes().iter().map(|n| n.kind).collect();
    assert_eq!(
        legacy_kinds,
        vec![
            TypeNodeKind::Map,
            TypeNodeKind::Name,
            TypeNodeKind::Slice,
            TypeNodeKind::Name,
        ]
    );
    assert!(matches!(
        parsed.tree_node(parsed.root().unwrap()).unwrap().kind,
        TypeTreeKind::Map { .. }
    ));
}

#[test]
fn malformed_recovery_never_panics() {
    let malformed_cases = [
        "",
        "*",
        "(",
        "(int",
        "[",
        "[N",
        "[N]",
        "map",
        "map[",
        "map[string]",
        "<-",
        "chan",
        "chan<-",
        "pkg.",
        "pkg.[int]",
        "M[]",
        "M[int",
        "M[int,]",
        "struct",
        "struct {",
        "struct { a, b }",
        "struct { a, *int }",
        "struct { []int }",
        "struct { * }",
        "struct { **int }",
        "struct { x ...int }",
        "struct { a [...]int }",
        "interface",
        "interface {",
        "interface { M(",
        "interface { ~ }",
        "interface { ~int | }",
        "func",
        "func(",
        "func(a int, b)",
        "func(*int, b string)",
        "func(...int, string)",
        "func(a, b ...int)",
        "func [T any](x T) T",
        "~T",
        "int + string",
    ];
    for case in malformed_cases {
        let parsed = parse_type(case);
        assert_tree_well_formed(case, &parsed);
        let constraint = parse_constraint(case);
        assert_tree_well_formed(case, &constraint);
        let prefix = parse_type_prefix(case);
        assert_tree_well_formed(case, &prefix);
    }

    // Deeply nested type hits depth limit cleanly without stack overflow.
    let deep = format!("{}int", "*".repeat(200));
    let parsed_deep = parse_type(&deep);
    assert!(!parsed_deep.diagnostics().is_empty());
    assert_tree_well_formed(&deep, &parsed_deep);
}

#[test]
fn array_length_nested_type_does_not_pollute_outer_tree() {
    let source = "struct { buf [len(struct{ y int }{})]byte; items [N]T; M[K, []V] }";
    let parsed = parse_type(source);
    assert!(
        parsed.diagnostics().is_empty(),
        "{:?}",
        parsed.diagnostics()
    );
    assert_tree_well_formed(source, &parsed);

    let root = parsed.root().unwrap();
    let TypeTreeKind::Struct { fields } = parsed.tree_node(root).unwrap().kind.clone() else {
        panic!("expected outer Struct");
    };
    let fields = parsed.struct_fields(fields);
    assert_eq!(fields.len(), 3);
    assert_eq!(slice(source, fields[0].name.as_ref().unwrap()), "buf");
    assert!(!fields[0].embedded);
    assert!(matches!(
        parsed.tree_node(fields[0].ty).unwrap().kind,
        TypeTreeKind::Array {
            len_expr: Some(_),
            ..
        }
    ));
    assert_eq!(slice(source, fields[1].name.as_ref().unwrap()), "items");
    assert!(!fields[1].embedded);
    assert!(matches!(
        parsed.tree_node(fields[1].ty).unwrap().kind,
        TypeTreeKind::Array {
            len_expr: Some(_),
            ..
        }
    ));
    assert_eq!(slice(source, fields[2].name.as_ref().unwrap()), "M");
    assert!(fields[2].embedded);
    assert!(matches!(
        parsed.tree_node(fields[2].ty).unwrap().kind,
        TypeTreeKind::Instantiation { .. }
    ));

    // Only the outer struct's 3 fields exist in `field_nodes` (the struct inside
    // the array-length expression does not leak into the outer type tree).
    assert_eq!(parsed.field_nodes().len(), 3);

    // Unnamed instantiation + array params in a function type.
    let fn_src = "func(M[K, []V], [N]int)";
    let fn_parsed = parse_type(fn_src);
    assert!(fn_parsed.diagnostics().is_empty());
    assert_tree_well_formed(fn_src, &fn_parsed);
    let TypeTreeKind::Func(sig_id) = fn_parsed.tree_node(fn_parsed.root().unwrap()).unwrap().kind
    else {
        panic!("expected Func");
    };
    let sig = fn_parsed.sig_node(sig_id).unwrap();
    let params = fn_parsed.sig_params(sig.params.clone());
    assert_eq!(params.len(), 2);
    assert!(params[0].name.is_none());
    assert!(matches!(
        fn_parsed.tree_node(params[0].ty).unwrap().kind,
        TypeTreeKind::Instantiation { .. }
    ));
    assert!(params[1].name.is_none());
    assert!(matches!(
        fn_parsed.tree_node(params[1].ty).unwrap().kind,
        TypeTreeKind::Array {
            len_expr: Some(_),
            ..
        }
    ));
}

#[test]
fn budget_overflow_clears_tree_cleanly() {
    // Build a union exceeding MAX_TYPE_NODES terms.
    let mut huge = String::with_capacity((MAX_TYPE_NODES + 10) * 3);
    for i in 0..=MAX_TYPE_NODES {
        if i > 0 {
            huge.push('|');
        }
        huge.push('T');
    }
    let parsed = parse_constraint(&huge);
    assert!(parsed.tree_overflowed());
    assert!(parsed.root().is_none());
    assert!(parsed.tree_nodes().is_empty());
    assert!(parsed.field_nodes().is_empty());
    assert!(parsed.interface_member_nodes().is_empty());
    assert!(parsed.sig_nodes().is_empty());
    assert!(parsed.param_nodes().is_empty());
    assert!(parsed.type_node_lists().is_empty());
    assert_tree_well_formed(&huge, &parsed);

    // Build a struct exceeding MAX_TYPE_NODES fields.
    let mut huge_struct = String::from("struct {");
    for _ in 0..=MAX_TYPE_NODES {
        huge_struct.push_str(" x int;");
    }
    huge_struct.push('}');
    let parsed_struct = parse_type(&huge_struct);
    assert!(parsed_struct.tree_overflowed());
    assert!(parsed_struct.root().is_none());
    assert_tree_well_formed(&huge_struct, &parsed_struct);
}
