//! Character classes, shared by the fuzzer (to pick characters) and by the
//! parameters (to compare which kinds of characters a string contains).

/// The broad kind of a character. Strings are compared on which classes they
/// contain, and the fuzzer builds strings from a random mix of classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CharClass {
    /// `a`–`z`
    Lowercase,
    /// `A`–`Z`
    Uppercase,
    /// `0`–`9`
    Digit,
    /// Printable ASCII symbols: `!`, `"`, `#`, ...
    Punctuation,
    /// ASCII whitespace: space, tab, newline, carriage return, form feed.
    Whitespace,
    /// Other ASCII control characters, e.g. NUL and DEL.
    Control,
    /// Anything outside ASCII: accented letters, CJK, emoji, ...
    NonAscii,
}

impl CharClass {
    /// Every class, in declaration order.
    pub const ALL: [CharClass; 7] = [
        CharClass::Lowercase,
        CharClass::Uppercase,
        CharClass::Digit,
        CharClass::Punctuation,
        CharClass::Whitespace,
        CharClass::Control,
        CharClass::NonAscii,
    ];
}

/// Which class a character belongs to. Every character is in exactly one.
pub fn classify(c: char) -> CharClass {
    if !c.is_ascii() {
        CharClass::NonAscii
    } else if c.is_ascii_lowercase() {
        CharClass::Lowercase
    } else if c.is_ascii_uppercase() {
        CharClass::Uppercase
    } else if c.is_ascii_digit() {
        CharClass::Digit
    } else if c.is_ascii_whitespace() {
        CharClass::Whitespace
    } else if c.is_ascii_control() {
        CharClass::Control
    } else {
        CharClass::Punctuation
    }
}
