use axum::http::HeaderMap;
use reqwest::Url;

/// Prefer the browser's public origin so server-generated links survive Host
/// rewrites across one or more reverse proxies.
pub fn base_url_from_request(headers: &HeaderMap, fallback: &str) -> String {
    ["origin", "referer"]
        .into_iter()
        .filter_map(|name| headers.get(name))
        .filter_map(|value| value.to_str().ok())
        .find_map(public_origin)
        .unwrap_or_else(|| fallback.trim_end_matches('/').to_string())
}

fn public_origin(value: &str) -> Option<String> {
    let url = Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }

    let origin = url.origin().ascii_serialization();
    (origin != "null").then_some(origin)
}

#[cfg(test)]
mod tests {
    use super::base_url_from_request;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn uses_the_browser_origin_before_the_configured_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("https://work.example.com"));
        assert_eq!(
            base_url_from_request(&headers, "http://192.168.50.10:16888"),
            "https://work.example.com"
        );
    }

    #[test]
    fn derives_an_origin_from_the_referrer_when_origin_is_unavailable() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "referer",
            HeaderValue::from_static("https://work.example.com/users?tab=requests"),
        );
        assert_eq!(
            base_url_from_request(&headers, "http://192.168.50.10:16888"),
            "https://work.example.com"
        );
    }

    #[test]
    fn rejects_non_http_origins_and_keeps_the_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert("origin", HeaderValue::from_static("file:///tmp/workbench"));
        assert_eq!(
            base_url_from_request(&headers, "http://192.168.50.10:16888"),
            "http://192.168.50.10:16888"
        );
    }
}
