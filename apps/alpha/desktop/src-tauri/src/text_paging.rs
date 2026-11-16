use thiserror::Error;

pub(crate) const MAX_PAGE_BYTES: usize = 64 * 1024;
const MIN_PAGE_BYTES: usize = 4;

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum TextPagingError {
    #[error("Page size must be between {MIN_PAGE_BYTES} and {MAX_PAGE_BYTES} bytes")]
    PageSizeOutOfRange,
    #[error("Page cursor is outside the text body")]
    CursorOutOfBounds,
    #[error("Page cursor does not point to a UTF-8 character boundary")]
    CursorNotOnCharacterBoundary,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TextPageSlice {
    pub(crate) content: String,
    pub(crate) next_cursor: Option<u64>,
}

pub(crate) fn page(
    text: &str,
    cursor: u64,
    max_bytes: u64,
) -> Result<TextPageSlice, TextPagingError> {
    let max_bytes = usize::try_from(max_bytes).map_err(|_| TextPagingError::PageSizeOutOfRange)?;
    if !(MIN_PAGE_BYTES..=MAX_PAGE_BYTES).contains(&max_bytes) {
        return Err(TextPagingError::PageSizeOutOfRange);
    }

    let cursor = usize::try_from(cursor).map_err(|_| TextPagingError::CursorOutOfBounds)?;
    if cursor > text.len() {
        return Err(TextPagingError::CursorOutOfBounds);
    }
    if !text.is_char_boundary(cursor) {
        return Err(TextPagingError::CursorNotOnCharacterBoundary);
    }

    let mut end = cursor.saturating_add(max_bytes).min(text.len());
    while end > cursor && !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == cursor && cursor < text.len() {
        return Err(TextPagingError::PageSizeOutOfRange);
    }

    Ok(TextPageSlice {
        content: text[cursor..end].to_owned(),
        next_cursor: (end < text.len()).then_some(end as u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_utf8_without_splitting_a_character() {
        let first = page("가나다라마바사", 0, 8).expect("first page");
        assert_eq!(first.content, "가나");
        assert_eq!(first.next_cursor, Some(6));

        let second = page("가나다라마바사", 6, 8).expect("second page");
        assert_eq!(second.content, "다라");
        assert_eq!(second.next_cursor, Some(12));
    }

    #[test]
    fn rejects_a_caller_supplied_mid_character_cursor() {
        assert_eq!(
            page("가나다", 1, 8),
            Err(TextPagingError::CursorNotOnCharacterBoundary)
        );
    }
}
