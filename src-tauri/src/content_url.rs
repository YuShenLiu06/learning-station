use crate::types::*;
pub fn validate(input: &str) -> Result<String> {
    let invalid = || CoreError::new("INVALID_INPUT", "请使用无凭据的公共 HTTPS 原链接。");
    let url = reqwest::Url::parse(input).map_err(|_| invalid())?;
    let host = url.host_str().ok_or_else(invalid)?;
    if input.len() > 2048
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
        || !host.contains('.')
        || host.contains(':')
        || host.parse::<std::net::IpAddr>().is_ok()
        || ["localhost", "local", "internal", "test", "invalid"]
            .iter()
            .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
    {
        return Err(invalid());
    }
    Ok(url.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_public_https_links() {
        for value in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://127.0.0.1/",
            "https://[::1]",
            "https://host.local",
            "https://user:pass@example.com",
            "https://example.com:9000",
        ] {
            assert!(validate(value).is_err());
        }
        assert_eq!(
            validate("https://example.com/watch?v=1").unwrap(),
            "https://example.com/watch?v=1"
        );
    }
}
