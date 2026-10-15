//! Request language detection for user-facing backend text.
//!
//! The frontend sends `Accept-Language: en` or `Accept-Language: zh-CN` on every
//! request. English is the default; any language tag starting with `zh` selects
//! Simplified Chinese.

use std::convert::Infallible;

use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, header::ACCEPT_LANGUAGE, request::Parts},
};

/// Language used for user-facing text in API responses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Zh,
}

impl Lang {
    /// Parse an `Accept-Language` header value.
    ///
    /// Only the first language tag is considered: a tag starting with `zh`
    /// (case-insensitive) selects Chinese, anything else (including an empty
    /// value) selects English.
    pub fn from_accept_language(value: &str) -> Lang {
        let first = value
            .split(',')
            .next()
            .and_then(|tag| tag.split(';').next())
            .map(str::trim)
            .unwrap_or("");

        if first.len() >= 2 && first[..2].eq_ignore_ascii_case("zh") { Lang::Zh } else { Lang::En }
    }

    /// Detect the language from request headers (`Accept-Language`).
    pub fn from_headers(headers: &HeaderMap) -> Lang {
        headers
            .get(ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok())
            .map(Lang::from_accept_language)
            .unwrap_or_default()
    }

    /// Pick the Chinese or English variant of a static string.
    pub fn pick<'a>(self, zh: &'a str, en: &'a str) -> &'a str {
        match self {
            Lang::Zh => zh,
            Lang::En => en,
        }
    }
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for Lang
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Lang::from_headers(&parts.headers))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn parses_accept_language() {
        assert_eq!(Lang::from_accept_language("zh-CN"), Lang::Zh);
        assert_eq!(Lang::from_accept_language("zh"), Lang::Zh);
        assert_eq!(Lang::from_accept_language("ZH-tw"), Lang::Zh);
        assert_eq!(Lang::from_accept_language("zh-CN,zh;q=0.9,en;q=0.8"), Lang::Zh);
        assert_eq!(Lang::from_accept_language(" zh-Hans ; q=1"), Lang::Zh);
        assert_eq!(Lang::from_accept_language("en"), Lang::En);
        assert_eq!(Lang::from_accept_language("en-US,zh-CN;q=0.9"), Lang::En);
        assert_eq!(Lang::from_accept_language("vi"), Lang::En);
        assert_eq!(Lang::from_accept_language("z"), Lang::En);
        assert_eq!(Lang::from_accept_language(""), Lang::En);
        assert_eq!(Lang::from_accept_language("*"), Lang::En);
    }

    #[test]
    fn parses_headers() {
        let mut headers = HeaderMap::new();
        assert_eq!(Lang::from_headers(&headers), Lang::En);
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("zh-CN"));
        assert_eq!(Lang::from_headers(&headers), Lang::Zh);
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en"));
        assert_eq!(Lang::from_headers(&headers), Lang::En);
    }

    #[test]
    fn picks_text() {
        assert_eq!(Lang::Zh.pick("中文", "English"), "中文");
        assert_eq!(Lang::En.pick("中文", "English"), "English");
        assert_eq!(Lang::default(), Lang::En);
    }
}
