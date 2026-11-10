//! Signature-help wire fields adapt Context callable selection.
use crate::analysis::query::{Context, SignatureTrigger};
use serde_json::Value;
use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, SignatureHelp, SignatureInformation,
};

pub(super) fn execute(
    context: &Context<'_>,
    offset: usize,
    params: &Value,
) -> Result<Value, (i32, String)> {
    let trigger = SignatureTrigger {
        explicit: params["context"]["triggerKind"].as_u64() == Some(1),
        retrigger: params["context"]["isRetrigger"].as_bool() == Some(true),
    };
    let Some(information) = context.signature_help(offset, trigger)? else {
        return Ok(Value::Null);
    };
    Ok(serde_json::to_value(SignatureHelp {
        active_parameter: information.active_parameter,
        active_signature: Some(0),
        signatures: vec![information.into()],
    })
    .expect("serializable signature help"))
}

impl From<crate::analysis::check::callable_signature::CallableSignature> for SignatureInformation {
    fn from(value: crate::analysis::check::callable_signature::CallableSignature) -> Self {
        Self {
            label: value.label,
            documentation: None,
            parameters: Some(
                value
                    .parameters
                    .into_iter()
                    .map(|parameter| ParameterInformation {
                        label: ParameterLabel::Simple(parameter.label),
                        documentation: None,
                    })
                    .collect(),
            ),
            active_parameter: value.active_parameter,
        }
    }
}
