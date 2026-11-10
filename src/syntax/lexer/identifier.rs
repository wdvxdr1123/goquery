//! Go identifiers use Unicode L and Nd, rather than derived alphabetic properties.
use super::unicode;

/// Whether a Go name begins with an uppercase (Unicode Lu) letter.
pub fn is_exported_name(name: &str) -> bool {
    name.chars().next().is_some_and(|character| {
        if character.is_ascii() {
            character.is_ascii_uppercase()
        } else {
            contains(unicode::UPPERCASE, character)
        }
    })
}

/// Whether a scalar may start a Go identifier (underscore or Unicode 17.0 L).
pub fn is_identifier_start(character: char) -> bool {
    if character.is_ascii() {
        character == '_' || character.is_ascii_alphabetic()
    } else {
        contains(unicode::LETTERS, character)
    }
}

/// Whether a scalar may continue a Go identifier (start characters or Nd).
pub fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || is_digit(character)
}

fn is_digit(character: char) -> bool {
    if character.is_ascii() {
        character.is_ascii_digit()
    } else {
        contains(unicode::DIGITS, character)
    }
}

fn contains(ranges: &[[u32; 2]], character: char) -> bool {
    let value = u32::from(character);
    let index = ranges.partition_point(|range| range[1] < value);
    ranges.get(index).is_some_and(|range| range[0] <= value)
}
