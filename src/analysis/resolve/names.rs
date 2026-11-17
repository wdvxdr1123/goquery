//! One lexical name used to prune candidates before native identity checking.
pub(crate) struct NameProbe {
    name: Box<str>,
}

impl NameProbe {
    pub(crate) fn new(name: &str) -> Self {
        Self { name: name.into() }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}
