use axum::http::HeaderMap;
use std::net::IpAddr;
use subtle::ConstantTimeEq;

fn parsed_ip(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .trim()
        .parse::<IpAddr>()
        .ok()
        .filter(|ip| !ip.is_unspecified() && !ip.is_multicast())
        .map(|ip| ip.to_canonical().to_string())
}

pub fn direct_ip(headers: &HeaderMap) -> Option<String> {
    parsed_ip(headers, "x-real-ip")
}

pub fn verified_ip(headers: &HeaderMap, expected_token: Option<&str>) -> Option<String> {
    let expected = expected_token.filter(|token| token.len() >= 32)?;
    let supplied = headers.get("x-crewdock-proxy-token")?.to_str().ok()?;
    if !bool::from(expected.as_bytes().ct_eq(supplied.as_bytes())) {
        return None;
    }
    parsed_ip(headers, "x-crewdock-client-ip")
}

pub fn client_ip(headers: &HeaderMap, expected_token: Option<&str>) -> Option<String> {
    verified_ip(headers, expected_token).or_else(|| direct_ip(headers))
}

#[cfg(test)]
mod tests {
    use super::{client_ip, verified_ip};
    use axum::http::{HeaderMap, HeaderValue};
    const TOKEN: &str = "test123456789012345678901234567890";

    #[test]
    fn only_verified_edge_headers_override_the_immediate_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("172.29.0.1"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9"));
        headers.insert(
            "x-crewdock-client-ip",
            HeaderValue::from_static("203.0.113.7"),
        );
        assert_eq!(
            client_ip(&headers, Some(TOKEN)).as_deref(),
            Some("172.29.0.1")
        );
        headers.insert("x-crewdock-proxy-token", HeaderValue::from_static("forged"));
        assert_eq!(
            client_ip(&headers, Some(TOKEN)).as_deref(),
            Some("172.29.0.1")
        );
        headers.insert("x-crewdock-proxy-token", HeaderValue::from_static(TOKEN));
        assert_eq!(
            client_ip(&headers, Some(TOKEN)).as_deref(),
            Some("203.0.113.7")
        );
        assert_eq!(client_ip(&headers, None).as_deref(), Some("172.29.0.1"));
    }

    #[test]
    fn invalid_or_multi_address_headers_never_authenticate_an_identity() {
        for value in ["203.0.113.1, 203.0.113.2", "unknown", "0.0.0.0", "ff02::1"] {
            let mut headers = HeaderMap::new();
            headers.insert("x-crewdock-proxy-token", HeaderValue::from_static(TOKEN));
            headers.insert("x-crewdock-client-ip", HeaderValue::from_str(value).unwrap());
            assert_eq!(verified_ip(&headers, Some(TOKEN)), None);
        }
    }
}
