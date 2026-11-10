//! Bounded Go module/workspace directive parsing, independent of LSP and filesystem I/O.
#[cfg(test)]
mod tests;
mod version;

pub use version::{compare_versions, escape_cache_path};
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Manifest {
    pub module: Option<String>,
    pub go: Option<String>,
    pub toolchain: Option<String>,
    pub excludes: Vec<(String, String)>,
    pub tools: Vec<String>,
    pub godebug: Vec<String>,
    pub retracts: Vec<Vec<String>>,
    pub ignores: Vec<String>,
    pub uses: Vec<String>,
    pub requires: Vec<(String, String)>,
    pub replacements: Vec<Replacement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Replacement {
    pub module: String,
    pub version: Option<String>,
    pub target: String,
    pub target_version: Option<String>,
}

pub fn parse(source: &str) -> Result<Manifest, String> {
    if source.len() > 8 * 1024 * 1024 {
        return Err("module file exceeds 8 MiB".into());
    }
    let mut result = Manifest::default();
    let mut block: Option<String> = None;
    let mut count = 0;
    for (index, line) in source.lines().enumerate() {
        let words = words(line).map_err(|error| format!("line {}: {error}", index + 1))?;
        if words.is_empty() {
            continue;
        }
        count += 1;
        if count > 16384 {
            return Err("module directive limit exceeded".into());
        }
        if words == [")"] {
            if block.take().is_none() {
                return Err("unexpected ')'".into());
            }
            continue;
        }
        let (directive, values) = if let Some(directive) = &block {
            (directive.as_str(), words.as_slice())
        } else {
            if words.len() == 2 && words[1] == "(" {
                if !matches!(
                    words[0].as_str(),
                    "require"
                        | "replace"
                        | "exclude"
                        | "retract"
                        | "use"
                        | "tool"
                        | "godebug"
                        | "ignore"
                ) {
                    return Err("invalid directive block".into());
                }
                block = Some(words[0].clone());
                continue;
            }
            (words[0].as_str(), &words[1..])
        };
        match directive {
            "module" if values.len() == 1 && result.module.is_none() => {
                result.module = Some(values[0].clone())
            }
            "go" if values.len() == 1 && result.go.is_none() => {
                result.go = Some(values[0].clone());
            }
            "toolchain" if values.len() == 1 && result.toolchain.is_none() => {
                result.toolchain = Some(values[0].clone());
            }
            "exclude" if values.len() == 2 => {
                result.excludes.push((values[0].clone(), values[1].clone()));
            }
            "tool" if values.len() == 1 => result.tools.push(values[0].clone()),
            "godebug" if values.len() == 1 => result.godebug.push(values[0].clone()),
            "retract" if !values.is_empty() => result.retracts.push(values.to_vec()),
            "ignore" if values.len() == 1 => result.ignores.push(values[0].clone()),
            "use" if values.len() == 1 => result.uses.push(values[0].clone()),
            "require" if values.len() == 2 => {
                result.requires.push((values[0].clone(), values[1].clone()))
            }
            "replace" => {
                let arrow = values
                    .iter()
                    .position(|word| word == "=>")
                    .ok_or("replace needs '=>'")?;
                let (left, rest) = values.split_at(arrow);
                let right = &rest[1..];
                if !(1..=2).contains(&left.len()) || !(1..=2).contains(&right.len()) {
                    return Err("invalid replace directive".into());
                }
                result.replacements.push(Replacement {
                    module: left[0].clone(),
                    version: left.get(1).cloned(),
                    target: right[0].clone(),
                    target_version: right.get(1).cloned(),
                });
            }
            "module" | "use" | "require" => return Err(format!("invalid {directive} directive")),
            _ => return Err(format!("invalid or unsupported {directive} directive")),
        }
    }
    if block.is_some() {
        return Err("unterminated directive block".into());
    }
    Ok(result)
}

fn words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut offset = 0;
    let bytes = line.as_bytes();
    while offset < bytes.len() {
        if bytes[offset].is_ascii_whitespace() {
            offset += 1;
            continue;
        }
        if line[offset..].starts_with("//") {
            break;
        }
        if line[offset..].starts_with("/*") {
            return Err("module comments must use //".into());
        }
        let start = offset;
        match bytes[offset] {
            b'(' | b')' => {
                offset += 1;
                words.push(line[start..offset].to_owned());
            }
            quote @ (b'"' | b'`') => {
                offset += 1;
                let mut closed = false;
                while offset < bytes.len() {
                    let byte = bytes[offset];
                    offset += 1;
                    if byte == quote {
                        closed = true;
                        break;
                    }
                    if quote == b'"' && byte == b'\\' {
                        offset += 1;
                    }
                }
                if !closed {
                    return Err("unterminated quoted word".into());
                }
                words.push(
                    crate::syntax::decode_import_path(&line[start..offset])
                        .ok_or("invalid quoted word")?,
                );
            }
            _ => {
                while offset < bytes.len()
                    && !bytes[offset].is_ascii_whitespace()
                    && !matches!(bytes[offset], b'(' | b')')
                    && !line[offset..].starts_with("//")
                {
                    if line[offset..].starts_with("/*") {
                        return Err("module comments must use //".into());
                    }
                    offset += line[offset..]
                        .chars()
                        .next()
                        .expect("nonempty word")
                        .len_utf8();
                }
                words.push(line[start..offset].to_owned());
            }
        }
    }
    Ok(words)
}
