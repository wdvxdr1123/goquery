//! External go/types differential entry point, compiled only for tests.
use crate::analysis::obj::{ObjectRef, SymbolOrigin};
use crate::analysis::sema::Semantics;
use crate::analysis::types::{ComplexPart, Constant, Mode};
use crate::workspace::{self, Workspace};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

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

    let mut sema = Semantics::new(&workspace);
    let mut current_file: Option<PathBuf> = None;
    let mut current_source = String::new();
    let mut rows = Vec::new();

    for entry in oracle["bindings"].as_array().unwrap() {
        let file = Path::new(entry["file"].as_str().unwrap());
        let offset = entry["offset"].as_u64().unwrap() as usize;
        let name = entry["name"].as_str().unwrap();
        if entry.get("scope").and_then(Value::as_str).unwrap_or("body") != "body" {
            rows.push(json!({
                "file": file,
                "offset": offset,
                "name": name,
                "type": Value::Null,
                "identity": Value::Null,
                "form": Value::Null,
                "initializer": Value::Null,
            }));
            continue;
        }

        if current_file.as_deref() != Some(file) {
            current_source = workspace.source(file).expect("indexed fixture source");
            current_file = Some(file.to_path_buf());
        }

        let interned_file = sema.intern_file(file).expect("indexed fixture file");
        let (ty, identity, form, initializer) = if let Some((region, arena)) =
            sema.region_and_arena_at(interned_file, &current_source, offset as u32, None)
        {
            let local_id = region.scopes.definition_at(offset as u32).or_else(|| {
                region
                    .implicits
                    .iter()
                    .find_map(|(span, obj)| (span[0] == offset as u32).then_some(*obj))
                    .and_then(|obj| match obj {
                        ObjectRef::Var {
                            origin: SymbolOrigin::Local { id, .. },
                            ..
                        } => Some(id),
                        _ => None,
                    })
            });
            let op = local_id.and_then(|id| region.local_type(id));
            let ty_id = op.map(|o| o.ty);
            let ty = ty_id.and_then(|t| arena.render(t, interned_file));
            let identity = ty_id.and_then(|t| arena.test_identity(t));
            let form = ty_id.and_then(|t| arena.form(t).map(|f| f.test_name()));
            let initializer = local_id
                .and_then(|id| region.local_init(id))
                .and_then(|init_span| {
                    let s = init_span[0] as usize;
                    let e = init_span[1] as usize;
                    if e.saturating_sub(s) > 16 * 1024 {
                        return None;
                    }
                    let expr_text = current_source.get(s..e)?;
                    let init_op = region.expr_type(init_span);
                    let numeric = init_op
                        .filter(|o| o.mode == Mode::Constant)
                        .and_then(|o| exact_numeric(o.value?));
                    Some(json!({"offset": s, "expr": expr_text, "numeric": numeric}))
                });
            (ty, identity, form, initializer)
        } else {
            (None, None, None, None)
        };

        rows.push(json!({
            "file": file,
            "offset": offset,
            "name": name,
            "type": ty,
            "identity": identity,
            "form": form,
            "initializer": initializer,
        }));
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
