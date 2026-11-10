use std::ops::Range;

#[derive(Clone, Copy)]
enum CharacterKind {
    Word,
    Whitespace,
    Newline,
    Other,
}

impl From<char> for CharacterKind {
    fn from(character: char) -> Self {
        if character == '_'
            || character.is_ascii_alphanumeric()
            || matches!(character, '\u{00C0}'..='\u{024F}' | '\u{0400}'..='\u{04FF}' | '\u{1E00}'..='\u{1EFF}' | '\u{0300}'..='\u{036F}')
        {
            Self::Word
        } else if matches!(character, '\n' | '\r') {
            Self::Newline
        } else if character.is_whitespace() {
            Self::Whitespace
        } else {
            Self::Other
        }
    }
}

pub(crate) fn word_range_from_chars(
    offset: usize,
    character: char,
    previous: impl Iterator<Item = char>,
    following: impl Iterator<Item = char>,
) -> Range<usize> {
    let kind = CharacterKind::from(character);
    let connects = |next| {
        matches!(
            (kind, CharacterKind::from(next)),
            (CharacterKind::Word, CharacterKind::Word)
                | (CharacterKind::Whitespace, CharacterKind::Whitespace)
        )
    };
    let start = previous
        .take(128)
        .take_while(|character| connects(*character))
        .fold(offset, |offset, character| offset - character.len_utf8());
    let end = following
        .take(128)
        .take_while(|character| connects(*character))
        .fold(offset + character.len_utf8(), |offset, character| {
            offset + character.len_utf8()
        });
    start..end
}
