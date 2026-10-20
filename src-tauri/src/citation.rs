//! Exact Unicode scalar offsets shared by material, message and exercise citations.
use crate::types::{CoreError, Result};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Locator {
    pub start_char: usize,
    pub end_char: usize,
}
pub fn validate_shape(selection: &str, locator: Option<&Locator>) -> Result<()> {
    if selection.is_empty() {
        return Err(CoreError::new(
            "INVALID_CONTEXT_REQUEST",
            "引用原文不能为空。",
        ));
    }
    if selection.chars().count() > 8000 || selection.len() > 30000 {
        return Err(CoreError::new(
            "LIMIT_EXCEEDED",
            "引用最多8000字符、30000字节。",
        ));
    }
    if locator
        .is_some_and(|l| l.end_char.checked_sub(l.start_char) != Some(selection.chars().count()))
    {
        return Err(CoreError::new(
            "INVALID_CONTEXT_REQUEST",
            "引用位置与原文长度不一致。",
        ));
    }
    Ok(())
}
pub fn validate_source(text: &str, selection: &str, locator: Option<&Locator>) -> Result<()> {
    validate_shape(selection, locator)?;
    let matches = match locator {
        Some(l) => text
            .chars()
            .skip(l.start_char)
            .take(l.end_char - l.start_char)
            .eq(selection.chars()),
        None => text.contains(selection), // Persisted legacy intent remains readable.
    };
    if !matches {
        return Err(CoreError::new(
            "CONTEXT_CONFLICT",
            "引用位置或原文已变化，请重新选择；原引用保留。",
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_duplicate_positions_and_forgery() {
        let text = "🙂相同句\n另一段\n相同句";
        let l = Locator {
            start_char: 9,
            end_char: 12,
        };
        validate_source(text, "相同句", Some(&l)).unwrap();
        assert_eq!(
            validate_source(
                text,
                "相同句",
                Some(&Locator {
                    start_char: 8,
                    end_char: 11
                })
            )
            .unwrap_err()
            .code,
            "CONTEXT_CONFLICT"
        );
        assert!(validate_source(
            text,
            "相同句",
            Some(&Locator {
                start_char: usize::MAX - 3,
                end_char: usize::MAX
            })
        )
        .is_err());
        assert!(validate_shape(
            "相同句",
            Some(&Locator {
                start_char: 4,
                end_char: 1
            })
        )
        .is_err());
        assert!(
            serde_json::from_str::<Locator>(r#"{"startChar":0,"endChar":1,"gitOid":"fake"}"#)
                .is_err()
        );
    }
}
