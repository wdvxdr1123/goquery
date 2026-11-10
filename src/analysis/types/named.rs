//! Lazy generic-owner bound checks; no declaration body or source is retained.
use super::constraints;
use super::{MAX_DEPTH, MAX_NODES, Ty, TypeArena, TypeId};

pub(super) fn parameter_header(source: &str, generic: bool) -> Option<String> {
    match crate::syntax::type_parameter_end(source) {
        Some(end) => Some(format!("func _{}()", source.get(..end)?)),
        None if !generic => Some("func _()".to_owned()),
        None => None,
    }
}

impl TypeArena<'_> {
    pub(super) fn check_named_arguments(&mut self, input: TypeId) -> Option<()> {
        if self.named_check_depth >= MAX_DEPTH {
            return None;
        }
        self.named_check_depth += 1;
        let result = self.check_named_arguments_inner(input);
        self.named_check_depth -= 1;
        result
    }

    fn check_named_arguments_inner(&mut self, input: TypeId) -> Option<()> {
        self.step()?;
        let Ty::Named {
            declaration,
            arguments,
        } = self.nodes[usize::from(input.0)].clone()
        else {
            return Some(());
        };
        if arguments.is_empty() && !declaration.symbol.generic {
            return Some(());
        }
        if let Some((_, valid)) = self.named_checks.iter().find(|(ty, _)| *ty == input) {
            return (*valid).then_some(());
        }
        if self.named_checks.len() >= MAX_NODES {
            return None;
        }
        let slot = self.named_checks.len();
        // Pending and failed checks cannot prove recursive bounds by themselves.
        self.named_checks.push((input, false));
        let source = self.workspace.source_slice(
            declaration.file,
            declaration.symbol.end..declaration.symbol.declaration()?.end,
        )?;
        let header = parameter_header(&source, declaration.symbol.generic)?;
        let parsed = crate::syntax::parse_signature(&header);
        if !parsed.diagnostics.is_empty() {
            return None;
        }
        let names = constraints::parameter_names(&header, &parsed)?;
        let arguments = self.arguments[arguments].to_vec();
        if names.len() != arguments.len() || names.len() > MAX_NODES {
            return None;
        }
        let environment = names
            .into_iter()
            .zip(arguments.iter().copied())
            .collect::<Vec<_>>();
        let bounds = self.constraints(declaration.file, &header, &parsed, &environment)?;
        for (bound, argument) in bounds.iter().zip(arguments) {
            self.prove_constraint(bound, argument, 0)?;
        }
        self.named_checks[slot].1 = true;
        Some(())
    }
}
