//! External go/types differential entry point, compiled only for tests.
use crate::analysis::check::Inference;
use crate::analysis::source::SourceAnalysis;
use crate::analysis::types::TypeArena;
use crate::analysis::types::{ComplexPart, Constant, Mode};
use crate::workspace::{self, Workspace};
use serde_json::{Value, json};
use std::path::Path;

#[test]
#[ignore = "run scripts/compare_types.py with pinned Go 1.27.3"]
fn type_oracle() {
    let input = std::env::var_os("GOQUERY_TYPE_ORACLE_INPUT").expect("oracle input path");
    let output = std::env::var_os("GOQUERY_TYPE_ORACLE_OUTPUT").expect("native output path");
    let oracle: Value =
        serde_json::from_slice(&std::fs::read(input).expect("read oracle")).unwrap();
    let mut workspace = Workspace::default();
    workspace
        .initialize(
            &json!({"rootUri": workspace::uri(Path::new(oracle["root"].as_str().unwrap()))}),
        )
        .unwrap();
    let mut rows = Vec::new();
    for entry in oracle["bindings"].as_array().unwrap() {
        let file = Path::new(entry["file"].as_str().unwrap());
        let source = workspace.source(file).expect("indexed fixture source");
        let analysis = SourceAnalysis::new(&source, &workspace.files[file]).unwrap();
        let offset = entry["offset"].as_u64().unwrap() as usize;
        let name = entry["name"].as_str().unwrap();
        let binding = analysis
            .locals
            .definition_at(&(offset..offset + name.len()), name);
        let mut ty = binding
            .and_then(|binding| workspace.binding_type(file, &source, &analysis.locals, binding));
        let mut arena = TypeArena::new(&workspace);
        let native = binding.and_then(|binding| {
            Inference::new(&workspace, file, &source, &analysis.locals, &[], 0)
                .test_binding(&mut arena, binding)
        });
        // A represented failure cannot acquire an identity via legacy spelling.
        let input = match native {
            Some(value) => value.map(|value| value.ty),
            None => ty.as_deref().and_then(|text| arena.parse(file, text)),
        };
        // Identity stays authoritative; when the legacy text boundary rejects an
        // ambiguous local spelling (shadowed formal/local type), the presentation
        // is the checker's declaration-scope rendering instead of nothing.
        if ty.is_none()
            && let Some(input) = input
        {
            ty = arena
                .render_resolved(input, file)
                .or_else(|| arena.render(input, file));
        }
        let identity = input.and_then(|input| arena.test_identity(input));
        let form = input.and_then(|input| arena.form(input).map(|form| form.test_name()));
        let initializer = binding.and_then(|binding| {
            let (span, result, count) = binding.initializer()?;
            if result != 0 || count != 1 || span.len() > 16 * 1024 {
                return None;
            }
            let text = source.get(span.clone())?;
            let parsed = crate::syntax::parse_expression(text);
            if !parsed.diagnostics.is_empty() {
                return None;
            }
            let inference = Inference::new(
                &workspace,
                file,
                &source,
                &analysis.locals,
                &parsed.nodes,
                span.start,
            );
            let mut arena = TypeArena::new(&workspace);
            let operand = inference.test_operand(&mut arena, parsed.root);
            let numeric = operand
                .filter(|operand| operand.mode == Mode::Constant)
                .and_then(|operand| exact_numeric(operand.value?));
            Some(json!({"offset":span.start,"expr":text,"numeric":numeric}))
        });
        rows.push(
            json!({"file": file, "offset": offset, "name": name, "type": ty, "identity": identity, "form": form, "initializer":initializer}),
        );
    }
    std::fs::write(output, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
}

fn exact_numeric(value: Constant) -> Option<Value> {
    let (real, imaginary) = match value {
        Constant::Int(value)
        | Constant::Float {
            integral: Some(value),
            ..
        } => (value, 0),
        Constant::Complex {
            real: ComplexPart::Integer(real),
            imaginary: ComplexPart::Integer(imaginary),
        } => (real, imaginary),
        _ => return None,
    };
    Some(json!({"real":real.to_string(),"imaginary":imaginary.to_string()}))
}
