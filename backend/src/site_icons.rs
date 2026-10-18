use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt;
use reqwest::{
    Client, Url,
    header::{ACCEPT, CONTENT_TYPE, HeaderMap, LOCATION},
    redirect::Policy,
};
use scraper::{Html, Selector};
use serde::Deserialize;
use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

const MAX_HTML_BYTES: usize = 256 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_ICON_BYTES: usize = 128 * 1024;
const MAX_CANDIDATES: usize = 12;

#[derive(Clone, Debug, Eq, PartialEq)]
struct IconCandidate {
    url: Url,
    score: u32,
}

#[derive(Debug, Deserialize)]
struct WebManifest {
    #[serde(default)]
    icons: Vec<ManifestIcon>,
}

#[derive(Debug, Deserialize)]
struct ManifestIcon {
    src: String,
    sizes: Option<String>,
    #[serde(rename = "type")]
    content_type: Option<String>,
}

pub async fn fetch_site_icon(site_url: &str, private_origins: &[String]) -> Option<String> {
    tokio::time::timeout(
        Duration::from_secs(5),
        fetch_site_icon_inner(site_url, private_origins),
    )
    .await
    .ok()
    .flatten()
}

async fn fetch_site_icon_inner(site_url: &str, private_origins: &[String]) -> Option<String> {
    let site_url = Url::parse(site_url).ok()?;
    if !is_http_url(&site_url) {
        return None;
    }
    let mut candidates = Vec::new();
    let mut manifest_urls = Vec::new();
    if let Some((headers, html, final_url)) = fetch_limited(
        private_origins,
        site_url.clone(),
        MAX_HTML_BYTES,
        "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1",
    )
    .await
    {
        if is_html_content(&headers) {
            let html = String::from_utf8_lossy(&html);
            let parsed = parse_document_links(&html, &final_url);
            candidates.extend(parsed.0);
            manifest_urls.extend(parsed.1);
        }
    }

    for manifest_url in manifest_urls.into_iter().take(2) {
        if let Some((headers, body, final_url)) = fetch_limited(
            private_origins,
            manifest_url,
            MAX_MANIFEST_BYTES,
            "application/manifest+json,application/json;q=0.9,*/*;q=0.1",
        )
        .await
        {
            if is_manifest_content(&headers) {
                candidates.extend(parse_manifest_icons(&body, &final_url));
            }
        }
    }

    if let Ok(favicon) = site_url.join("/favicon.ico") {
        candidates.push(IconCandidate {
            url: favicon,
            score: 1,
        });
    }
    deduplicate_and_sort(&mut candidates);

    for candidate in candidates.into_iter().take(MAX_CANDIDATES) {
        let Some((headers, bytes, _)) = fetch_limited(
            private_origins,
            candidate.url,
            MAX_ICON_BYTES,
            "image/avif,image/webp,image/png,image/jpeg,image/gif,image/x-icon,image/vnd.microsoft.icon;q=0.9,*/*;q=0.1",
        )
        .await
        else {
            continue;
        };
        let declared = content_type(&headers);
        if let Some(mime) = detect_image_mime(&bytes, declared) {
            return Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes)));
        }
    }
    None
}

async fn fetch_limited(
    private_origins: &[String],
    mut url: Url,
    limit: usize,
    accept: &str,
) -> Option<(HeaderMap, Vec<u8>, Url)> {
    let mut final_response = None;
    for redirects in 0..=2 {
        if !is_http_url(&url) {
            return None;
        }
        let port = url.port_or_known_default()?;
        let host = url.host_str()?;
        let addresses: Vec<SocketAddr> = match host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
        {
            Ok(ip) => vec![SocketAddr::new(ip, port)],
            Err(_) => tokio::net::lookup_host((host, port)).await.ok()?.collect(),
        };
        if !addresses_allowed(&url, &addresses, private_origins) {
            return None;
        }
        // Pin the validated DNS result. Disable environment proxies and
        // revalidate each redirect, manifest and icon destination.
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(4))
            .redirect(Policy::none())
            .resolve_to_addrs(url.host_str()?, &addresses)
            .user_agent("crewdock-icon-fetcher/1.0")
            .build()
            .ok()?;
        let response = client
            .get(url.clone())
            .header(ACCEPT, accept)
            .send()
            .await
            .ok()?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            if redirects == 2 {
                return None;
            }
            url = url
                .join(response.headers().get(LOCATION)?.to_str().ok()?)
                .ok()?;
            continue;
        }
        final_response = Some(response);
        break;
    }
    let response = final_response?;
    if !response.status().is_success() {
        return None;
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return None;
    }
    let final_url = response.url().clone();
    if !is_http_url(&final_url) {
        return None;
    }
    let headers = response.headers().clone();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if exceeds_size(bytes.len(), chunk.len(), limit) {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some((headers, bytes, final_url))
}

fn exceeds_size(current: usize, incoming: usize, limit: usize) -> bool {
    current
        .checked_add(incoming)
        .is_none_or(|total| total > limit)
}

fn parse_document_links(html: &str, base_url: &Url) -> (Vec<IconCandidate>, Vec<Url>) {
    let document = Html::parse_document(html);
    let selector = Selector::parse("link[href]").expect("valid icon selector");
    let mut icons = Vec::new();
    let mut manifests = Vec::new();
    for link in document.select(&selector) {
        let rel = link.value().attr("rel").unwrap_or("").to_ascii_lowercase();
        let rel_tokens = rel.split_ascii_whitespace().collect::<Vec<_>>();
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Some(url) = resolve_http_url(base_url, href) else {
            continue;
        };
        if rel_tokens.contains(&"manifest") {
            manifests.push(url);
            continue;
        }
        let is_apple = rel_tokens
            .iter()
            .any(|token| token.starts_with("apple-touch-icon"));
        let is_icon = rel_tokens.contains(&"icon") || rel_tokens.contains(&"shortcut");
        if !is_apple && !is_icon {
            continue;
        }
        let size = icon_size_score(link.value().attr("sizes"));
        let base_score = if is_apple { 500 } else { 200 };
        icons.push(IconCandidate {
            url,
            score: base_score + size,
        });
    }
    deduplicate_and_sort(&mut icons);
    manifests.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    manifests.dedup();
    (icons, manifests)
}

fn parse_manifest_icons(body: &[u8], base_url: &Url) -> Vec<IconCandidate> {
    let Ok(manifest) = serde_json::from_slice::<WebManifest>(body) else {
        return Vec::new();
    };
    let mut candidates = manifest
        .icons
        .into_iter()
        .filter(|icon| {
            !icon
                .content_type
                .as_deref()
                .is_some_and(|content_type| content_type.to_ascii_lowercase().contains("svg"))
        })
        .filter_map(|icon| {
            resolve_http_url(base_url, &icon.src).map(|url| IconCandidate {
                url,
                score: 400 + icon_size_score(icon.sizes.as_deref()),
            })
        })
        .collect::<Vec<_>>();
    deduplicate_and_sort(&mut candidates);
    candidates
}

fn deduplicate_and_sort(candidates: &mut Vec<IconCandidate>) {
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.url.as_str().cmp(right.url.as_str()))
    });
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.url.as_str().to_string()));
}

fn icon_size_score(sizes: Option<&str>) -> u32 {
    sizes
        .unwrap_or("")
        .split_ascii_whitespace()
        .filter_map(|size| size.split_once('x'))
        .filter_map(|(width, height)| Some(width.parse::<u32>().ok()?.min(height.parse().ok()?)))
        .max()
        .unwrap_or(0)
        .min(512)
}

fn resolve_http_url(base_url: &Url, value: &str) -> Option<Url> {
    let url = base_url.join(value.trim()).ok()?;
    is_http_url(&url).then_some(url)
}

fn is_http_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

pub fn parse_private_origins(value: &str) -> anyhow::Result<Vec<String>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let url = Url::parse(value)?;
            anyhow::ensure!(
                is_http_url(&url)
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "icon private origins must be exact HTTP(S) origins"
            );
            Ok(url.origin().ascii_serialization())
        })
        .collect()
}

fn ipv4_is_public(ip: Ipv4Addr) -> bool {
    let value = u32::from(ip);
    let blocked = [
        (0x00000000, 8),
        (0x0a000000, 8),
        (0x64400000, 10),
        (0x7f000000, 8),
        (0xa9fe0000, 16),
        (0xac100000, 12),
        (0xc0000000, 24),
        (0xc0000200, 24),
        (0xc0586300, 24),
        (0xc0a80000, 16),
        (0xc6120000, 15),
        (0xc6336400, 24),
        (0xcb007100, 24),
        (0xe0000000, 4),
        (0xf0000000, 4),
    ];
    !blocked
        .iter()
        .any(|(network, prefix)| value & (u32::MAX << (32 - prefix)) == *network)
}

fn destination_allowed(ip: IpAddr, allow_private: bool) -> bool {
    match ip {
        IpAddr::V4(ip) => ipv4_is_public(ip) || (allow_private && ip.is_private()),
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return destination_allowed(mapped.into(), allow_private);
            }
            let s = ip.segments();
            let public = s[0] & 0xe000 == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] & 0xf000 == 0);
            public || (allow_private && s[0] & 0xfe00 == 0xfc00)
        }
    }
}

fn addresses_allowed(url: &Url, addresses: &[SocketAddr], private_origins: &[String]) -> bool {
    let allow_private = private_origins.contains(&url.origin().ascii_serialization());
    !addresses.is_empty()
        && addresses
            .iter()
            .all(|address| destination_allowed(address.ip(), allow_private))
}

fn content_type(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim())
}

fn is_html_content(headers: &HeaderMap) -> bool {
    content_type(headers).is_none_or(|value| {
        matches!(
            value,
            "text/html" | "application/xhtml+xml" | "application/octet-stream"
        )
    })
}

fn is_manifest_content(headers: &HeaderMap) -> bool {
    content_type(headers).is_none_or(|value| {
        matches!(
            value,
            "application/manifest+json" | "application/json" | "text/json" | "text/plain"
        )
    })
}

fn detect_image_mime(bytes: &[u8], declared: Option<&str>) -> Option<&'static str> {
    let declared = declared.unwrap_or("").to_ascii_lowercase();
    if declared.contains("svg") || looks_like_svg(bytes) {
        return None;
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(&[0, 0, 1, 0]) {
        return Some("image/x-icon");
    }
    None
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]).to_ascii_lowercase();
    let prefix = prefix
        .trim_start_matches(|character: char| character.is_whitespace() || character == '\u{feff}');
    prefix.starts_with("<svg") || (prefix.starts_with("<?xml") && prefix.contains("<svg"))
}

#[cfg(test)]
mod tests {
    use super::{
        addresses_allowed, destination_allowed, detect_image_mime, exceeds_size, is_http_url,
        parse_document_links, parse_private_origins,
    };
    use reqwest::Url;

    #[test]
    fn server_icon_requests_reject_special_addresses_including_mapped_ipv6() {
        for ip in [
            "0.0.0.0",
            "127.0.0.1",
            "10.0.0.1",
            "172.16.1.2",
            "192.168.1.1",
            "169.254.169.254",
            "100.100.100.200",
            "198.18.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "2001:db8::1",
            "3fff::1",
        ] {
            assert!(!destination_allowed(ip.parse().unwrap(), false), "{ip}");
        }
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(destination_allowed(ip.parse().unwrap(), false), "{ip}");
        }
    }

    #[test]
    fn allowlist_is_exact_origin_and_never_allows_loopback_or_metadata() {
        let allowed =
            parse_private_origins("http://192.168.1.10:8080, https://nas.internal").unwrap();
        let target = Url::parse("http://192.168.1.10:8080/favicon.ico").unwrap();
        let addresses = ["192.168.1.10:8080".parse().unwrap()];
        assert!(addresses_allowed(&target, &addresses, &allowed));
        assert!(!addresses_allowed(
            &Url::parse("http://192.168.1.10:9000/").unwrap(),
            &addresses,
            &allowed
        ));
        assert!(!addresses_allowed(
            &Url::parse("http://elsewhere.internal/").unwrap(),
            &addresses,
            &allowed
        ));
        for ip in [
            "127.0.0.1",
            "169.254.169.254",
            "100.100.100.200",
            "::1",
            "fe80::1",
        ] {
            assert!(!destination_allowed(ip.parse().unwrap(), true));
        }
        assert!(parse_private_origins("http://user:password@nas.internal").is_err());
        assert!(parse_private_origins("http://nas.internal/path").is_err());
    }

    #[test]
    fn mixed_public_private_dns_answers_and_credential_urls_are_rejected() {
        let url = Url::parse("https://example.com").unwrap();
        assert!(!addresses_allowed(&url, &[], &[]));
        assert!(!addresses_allowed(
            &url,
            &[
                "8.8.8.8:443".parse().unwrap(),
                "127.0.0.1:443".parse().unwrap()
            ],
            &[]
        ));
        assert!(!is_http_url(
            &Url::parse("http://user:pass@example.com").unwrap()
        ));
    }

    #[tokio::test]
    async fn blocked_literal_targets_fail_without_connecting() {
        for target in [
            "http://127.0.0.1:80",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
        ] {
            assert!(
                super::fetch_limited(&[], Url::parse(target).unwrap(), 1024, "*/*")
                    .await
                    .is_none()
            );
        }
    }

    #[test]
    fn document_icons_prefer_large_apple_icons() {
        let html = r#"
          <link rel="icon" sizes="32x32" href="/favicon-32.png">
          <link rel="apple-touch-icon" sizes="180x180" href="/touch.png">
          <link rel="icon" sizes="128x128" href="/favicon-128.png">
        "#;
        let base = Url::parse("https://example.com/path/page").unwrap();
        let (icons, _) = parse_document_links(html, &base);
        assert_eq!(icons[0].url.as_str(), "https://example.com/touch.png");
        assert_eq!(icons[1].url.as_str(), "https://example.com/favicon-128.png");
    }

    #[test]
    fn document_links_reject_non_http_icon_urls() {
        let html = r#"
          <link rel="icon" href="data:image/png;base64,abc">
          <link rel="icon" href="javascript:alert(1)">
          <link rel="icon" href="/safe.png">
        "#;
        let base = Url::parse("https://example.com/").unwrap();
        let (icons, _) = parse_document_links(html, &base);
        assert_eq!(icons.len(), 1);
        assert_eq!(icons[0].url.as_str(), "https://example.com/safe.png");
    }

    #[test]
    fn image_detection_accepts_supported_magic_bytes_and_rejects_svg() {
        assert_eq!(
            detect_image_mime(b"\x89PNG\r\n\x1a\nrest", Some("application/octet-stream")),
            Some("image/png")
        );
        assert_eq!(
            detect_image_mime(
                b"<svg xmlns='http://www.w3.org/2000/svg'/>",
                Some("image/svg+xml")
            ),
            None
        );
        assert_eq!(detect_image_mime(b"not an image", Some("image/png")), None);
    }

    #[test]
    fn response_size_limit_accepts_exact_limit_and_rejects_overflow() {
        assert!(!exceeds_size(64, 64, 128));
        assert!(exceeds_size(64, 65, 128));
        assert!(exceeds_size(usize::MAX, 1, usize::MAX));
    }
}
