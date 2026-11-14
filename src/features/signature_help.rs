//! Signature-help wire fields adapt Context callable selection.
use crate::analysis::query::{Context, SignatureTrigger};
use serde_json::Value;
use tower_lsp::lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureInformation,
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
    let text = information.documentation.unwrap_or_default();
    let (kind, text) = if context.workspace().signature_plaintext {
        (MarkupKind::PlainText, text)
    } else {
        let mut rendered = super::documentation::markdown(&text, &[]);
        if rendered.ends_with('\n') {
            rendered.pop();
        }
        (MarkupKind::Markdown, rendered)
    };
    let active_parameter = information.signature.active_parameter;
    let mut signature: SignatureInformation = information.signature.into();
    signature.documentation = Some(Documentation::MarkupContent(MarkupContent {
        kind,
        value: text,
    }));
    Ok(serde_json::to_value(SignatureHelp {
        active_parameter,
        active_signature: Some(0),
        signatures: vec![signature],
    })
    .expect("serializable signature help"))
}

impl From<crate::analysis::check::callable_signature::CallableSignature> for SignatureInformation {
    fn from(value: crate::analysis::check::callable_signature::CallableSignature) -> Self {
        Self {
            label: value.label,
            documentation: None,
            parameters: (!value.parameters.is_empty()).then(|| {
                value
                    .parameters
                    .into_iter()
                    .map(|parameter| ParameterInformation {
                        label: ParameterLabel::Simple(parameter.label),
                        documentation: None,
                    })
                    .collect()
            }),
            active_parameter: value.active_parameter,
        }
    }
}
