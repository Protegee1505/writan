//! S3-compatible sync target (ADR-0006).
//!
//! One bucket is another create-only store: a write goes out as a `PUT` with
//! `If-None-Match: *`, so a retry that hits an existing object gets `412` and
//! compares bytes — identical means the retry succeeded, different means
//! [`TargetError::Diverged`], exactly like
//! [`FolderTarget`](crate::target::FolderTarget) with `create_new`.
//!
//! Requests are signed with hand-rolled AWS Signature Version 4; the unit
//! tests pin the signer to vectors from AWS's own `aws-sig-v4-test-suite`.
//! The HTTP layer is a small blocking `ureq` call, which keeps the crate free
//! of an async runtime. Listing uses `ListObjectsV2` with pagination.

use std::fmt;
use std::io;

use hmac::{Hmac, Mac};
use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};
use sha2::{Digest, Sha256};

use crate::target::{SyncTarget, TargetError};

type HmacSha256 = Hmac<Sha256>;

/// Content type of every object the engine writes.
const CONTENT_TYPE: &str = "application/octet-stream";

/// Number of list pages after which we give up on a runaway bucket.
const MAX_LIST_PAGES: usize = 1024;

/// Connection settings for an S3-compatible store.
///
/// The endpoint is whole (`http://127.0.0.1:9000` for MinIO,
/// `https://s3.eu-central-1.amazonaws.com` for AWS); the target always uses
/// path-style addressing, which both accept.
#[derive(Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    /// Never persisted in `config.json` — the app keeps it in the OS keychain
    /// (ADR-0006 §6).
    pub secret_key: String,
}

impl S3Config {
    fn host(&self) -> String {
        let trimmed = self.endpoint.trim_end_matches('/');
        trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .unwrap_or(trimmed)
            .to_string()
    }

    fn url(&self, uri_path: &str, query: &str) -> String {
        let mut url = format!("{}{uri_path}", self.endpoint.trim_end_matches('/'));
        if !query.is_empty() {
            url.push('?');
            url.push_str(query);
        }
        url
    }
}

impl fmt::Debug for S3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key", &self.access_key)
            .field("secret_key", &"<redacted>")
            .finish()
    }
}

/// One page of a `ListObjectsV2` response.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ListPage {
    pub keys: Vec<String>,
    /// Present while more pages follow.
    pub next: Option<String>,
}

/// The HTTP seam of the S3 target.
///
/// Each method maps to one REST call the target needs, so the object-level
/// rules (412-compare, pagination) are testable against a canned transport and
/// the live path is exercised by the nightly simulator against MinIO.
pub trait S3Transport: Send + Sync {
    /// `PUT` with `If-None-Match: *`. `Ok(true)` means written, `Ok(false)`
    /// means the object already existed (bytes whatever they are).
    fn put_if_absent(&self, path: &str, bytes: &[u8]) -> Result<bool, io::Error>;
    /// `GET`; `Ok(None)` for 404.
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, io::Error>;
    /// One listing page.
    fn list_page(&self, prefix: &str, token: Option<&str>) -> Result<ListPage, io::Error>;
}

/// [`SyncTarget`] over an S3-compatible bucket.
pub struct S3Target<T: S3Transport> {
    transport: T,
}

impl<T: S3Transport> S3Target<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: S3Transport> SyncTarget for S3Target<T> {
    fn list(&self) -> Result<Vec<String>, TargetError> {
        let mut keys = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_LIST_PAGES {
            let page = self.transport.list_page("", token.as_deref())?;
            keys.extend(page.keys);
            match page.next {
                Some(next) if !next.is_empty() => token = Some(next),
                _ => return Ok(keys),
            }
        }
        Err(TargetError::Io(io::Error::other(
            "s3 listing did not finish within 1024 pages",
        )))
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, TargetError> {
        match self.transport.get(path)? {
            Some(bytes) => Ok(bytes),
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("s3 object not found: {path}"),
            )
            .into()),
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), TargetError> {
        if self.transport.put_if_absent(path, bytes)? {
            return Ok(());
        }
        match self.transport.get(path)? {
            Some(existing) if existing == bytes => Ok(()),
            _ => Err(TargetError::Diverged(path.to_string())),
        }
    }
}

/// The live transport: blocking `ureq` requests signed with SigV4.
pub struct S3HttpTransport {
    cfg: S3Config,
    agent: ureq::Agent,
}

/// Request parts that do not depend on the parameters: URL, canonical
/// pieces, timestamp and payload hash.
struct RequestPlan {
    url: String,
    canonical_uri: String,
    canonical_query: String,
    date_stamp: String,
    amz_date: String,
    payload_hash: String,
}

impl S3HttpTransport {
    pub fn new(cfg: S3Config) -> Self {
        Self {
            cfg,
            agent: ureq::Agent::new_with_defaults(),
        }
    }

    fn plan(&self, path: &str, query: &[(String, String)], body: &[u8]) -> RequestPlan {
        let mut pairs = query.to_vec();
        pairs.sort();
        let canonical_query = pairs
            .iter()
            .map(|(key, value)| format!("{}={}", uri_encode(key, true), uri_encode(value, true)))
            .collect::<Vec<_>>()
            .join("&");
        let encoded_path = uri_encode(path, false);
        let canonical_uri = if encoded_path.is_empty() {
            format!("/{}", self.cfg.bucket)
        } else {
            format!("/{}/{}", self.cfg.bucket, encoded_path)
        };
        let now = time::OffsetDateTime::now_utc();
        let date_stamp = format!(
            "{:04}{:02}{:02}",
            now.year(),
            u8::from(now.month()),
            now.day()
        );
        let amz_date = format!(
            "{date_stamp}T{:02}{:02}{:02}Z",
            now.hour(),
            now.minute(),
            now.second()
        );
        RequestPlan {
            url: self.cfg.url(&canonical_uri, &canonical_query),
            canonical_uri,
            canonical_query,
            date_stamp,
            amz_date,
            payload_hash: if body.is_empty() {
                EMPTY_SHA256.to_string()
            } else {
                sha256_hex(body)
            },
        }
    }

    fn authorization(
        &self,
        method: &str,
        plan: &RequestPlan,
        content_type: Option<&str>,
    ) -> String {
        let mut headers = vec![
            ("host".to_string(), self.cfg.host()),
            (
                "x-amz-content-sha256".to_string(),
                plan.payload_hash.clone(),
            ),
            ("x-amz-date".to_string(), plan.amz_date.clone()),
        ];
        if let Some(content_type) = content_type {
            headers.push(("content-type".to_string(), content_type.to_string()));
        }
        sign_request(
            method,
            &plan.canonical_uri,
            &plan.canonical_query,
            &headers,
            &plan.payload_hash,
            &SigV4 {
                date_stamp: &plan.date_stamp,
                amz_date: &plan.amz_date,
                region: &self.cfg.region,
                service: "s3",
                access_key: &self.cfg.access_key,
                secret_key: &self.cfg.secret_key,
            },
        )
    }

    fn put_object(&self, path: &str, bytes: &[u8]) -> Result<bool, io::Error> {
        let plan = self.plan(path, &[], bytes);
        let authorization = self.authorization("PUT", &plan, Some(CONTENT_TYPE));
        let response = self
            .agent
            .put(&plan.url)
            .header("x-amz-content-sha256", plan.payload_hash.as_str())
            .header("x-amz-date", plan.amz_date.as_str())
            .header("content-type", CONTENT_TYPE)
            .header("if-none-match", "*")
            .header("authorization", authorization.as_str())
            .send(bytes);
        match response {
            Ok(_) => Ok(true),
            Err(ureq::Error::StatusCode(412)) => Ok(false),
            Err(error) => Err(io::Error::other(format!("s3 put {path}: {error}"))),
        }
    }

    fn get_object(&self, path: &str) -> Result<Option<Vec<u8>>, io::Error> {
        let plan = self.plan(path, &[], &[]);
        let authorization = self.authorization("GET", &plan, None);
        let response = self
            .agent
            .get(&plan.url)
            .header("x-amz-content-sha256", plan.payload_hash.as_str())
            .header("x-amz-date", plan.amz_date.as_str())
            .header("authorization", authorization.as_str())
            .call();
        match response {
            Ok(mut response) => {
                let body = response
                    .body_mut()
                    .read_to_vec()
                    .map_err(|error| io::Error::other(format!("s3 get {path}: {error}")))?;
                Ok(Some(body))
            }
            Err(ureq::Error::StatusCode(404)) => Ok(None),
            Err(error) => Err(io::Error::other(format!("s3 get {path}: {error}"))),
        }
    }

    fn list_objects(&self, prefix: &str, token: Option<&str>) -> Result<ListPage, io::Error> {
        let mut query = vec![
            ("list-type".to_string(), "2".to_string()),
            ("max-keys".to_string(), "1000".to_string()),
            ("prefix".to_string(), prefix.to_string()),
        ];
        if let Some(token) = token {
            query.push(("continuation-token".to_string(), token.to_string()));
        }
        let plan = self.plan("", &query, &[]);
        let authorization = self.authorization("GET", &plan, None);
        let response = self
            .agent
            .get(&plan.url)
            .header("x-amz-content-sha256", plan.payload_hash.as_str())
            .header("x-amz-date", plan.amz_date.as_str())
            .header("authorization", authorization.as_str())
            .call();
        match response {
            Ok(mut response) => {
                let body = response
                    .body_mut()
                    .read_to_vec()
                    .map_err(|error| io::Error::other(format!("s3 list: {error}")))?;
                let text = String::from_utf8(body)
                    .map_err(|error| io::Error::other(format!("s3 list: not utf-8: {error}")))?;
                parse_list_page(&text)
            }
            Err(error) => Err(io::Error::other(format!("s3 list: {error}"))),
        }
    }
}

impl S3Transport for S3HttpTransport {
    fn put_if_absent(&self, path: &str, bytes: &[u8]) -> Result<bool, io::Error> {
        self.put_object(path, bytes)
    }

    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, io::Error> {
        self.get_object(path)
    }

    fn list_page(&self, prefix: &str, token: Option<&str>) -> Result<ListPage, io::Error> {
        self.list_objects(prefix, token)
    }
}

/// Pull the `Key`s and the `NextContinuationToken` out of a `ListObjectsV2`
/// answer. Unknown elements are skipped, so newer S3 fields cannot break us.
fn parse_list_page(body: &str) -> Result<ListPage, io::Error> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut page = ListPage::default();
    let mut in_contents = false;
    let mut capture: Option<&'static str> = None;
    let mut buffer = String::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|error| io::Error::other(format!("s3 list xml: {error}")))?;
        match event {
            Event::Eof => break,
            Event::Start(start) => {
                let name = start.name().0;
                capture = match name {
                    "Contents" => {
                        in_contents = true;
                        None
                    }
                    "Key" if in_contents => Some("Key"),
                    "NextContinuationToken" => Some("NextContinuationToken"),
                    _ => None,
                };
                buffer.clear();
            }
            Event::End(end) => {
                let name = end.name().0;
                if capture == Some("Key") && name == "Key" {
                    page.keys.push(std::mem::take(&mut buffer));
                } else if capture == Some("NextContinuationToken")
                    && name == "NextContinuationToken"
                {
                    let token = std::mem::take(&mut buffer);
                    page.next = if token.is_empty() { None } else { Some(token) };
                }
                if name == "Contents" {
                    in_contents = false;
                }
                capture = None;
            }
            Event::Text(text) if capture.is_some() => {
                buffer.push_str(&text.xml_content(XmlVersion::Implicit1_0));
            }
            _ => {}
        }
    }
    Ok(page)
}

// ---- AWS Signature Version 4 -------------------------------------------------

/// SHA-256 of the empty string: the payload hash of every bodyless request.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Percent-encode for SigV4: everything survives except `A-Za-z0-9-._~`, and
/// `/` survives when `encode_slash` is false (S3 object paths).
fn uri_encode(value: &str, encode_slash: bool) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let keep = matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~')
            || (byte == b'/' && !encode_slash);
        if keep {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(DIGITS[(byte >> 4) as usize] as char);
            out.push(DIGITS[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

/// Canonical header values are trimmed and inner whitespace runs collapse.
fn normalize_space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Everything SigV4 derives from context, rolled into one struct to keep the
/// signer's parameter list readable.
struct SigV4<'a> {
    date_stamp: &'a str,
    amz_date: &'a str,
    region: &'a str,
    service: &'a str,
    access_key: &'a str,
    secret_key: &'a str,
}

fn signing_key(secret_key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let key = hmac_sha256(
        format!("AWS4{secret_key}").as_bytes(),
        date_stamp.as_bytes(),
    );
    let key = hmac_sha256(&key, region.as_bytes());
    let key = hmac_sha256(&key, service.as_bytes());
    hmac_sha256(&key, b"aws4_request")
}

/// Build the `Authorization` header for a signed request. `headers` is
/// whatever will actually be sent (unsorted is fine).
fn sign_request(
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    headers: &[(String, String)],
    payload_hash: &str,
    signer: &SigV4<'_>,
) -> String {
    let mut normalized: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), normalize_space(value)))
        .collect();
    normalized.sort();
    let canonical_headers = normalized
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect::<String>();
    let signed_headers = normalized
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let scope = format!(
        "{}/{}/{}/aws4_request",
        signer.date_stamp, signer.region, signer.service
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        signer.amz_date,
        sha256_hex(canonical_request.as_bytes())
    );
    let key = signing_key(
        signer.secret_key,
        signer.date_stamp,
        signer.region,
        signer.service,
    );
    let signature = hex(&hmac_sha256(&key, string_to_sign.as_bytes()));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        signer.access_key
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Mutex;

    // The example credentials of the aws-sig-v4-test-suite.
    const ACCESS: &str = "AKIDEXAMPLE";
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    fn example_signer(service: &str) -> SigV4<'_> {
        SigV4 {
            date_stamp: "20150830",
            amz_date: "20150830T123600Z",
            region: "us-east-1",
            service,
            access_key: ACCESS,
            secret_key: SECRET,
        }
    }

    fn signature_of(authorization: &str) -> &str {
        authorization
            .rsplit_once("Signature=")
            .expect("authorization carries a signature")
            .1
    }

    #[test]
    fn sigv4_matches_the_aws_get_vanilla_vector() {
        // aws-sig-v4-test-suite: get-vanilla
        let headers = vec![
            ("host".to_string(), "example.amazonaws.com".to_string()),
            ("x-amz-date".to_string(), "20150830T123600Z".to_string()),
        ];
        let authorization = sign_request(
            "GET",
            "/",
            "",
            &headers,
            EMPTY_SHA256,
            &example_signer("service"),
        );
        assert_eq!(
            signature_of(&authorization),
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        assert!(authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request"
        ));
        assert!(authorization.contains("SignedHeaders=host;x-amz-date"));
    }

    #[test]
    fn sigv4_matches_the_aws_sorted_query_vector() {
        // aws-sig-v4-test-suite: get-vanilla-query-order-key-case; the
        // canonical query string is sorted by key.
        let headers = vec![
            ("host".to_string(), "example.amazonaws.com".to_string()),
            ("x-amz-date".to_string(), "20150830T123600Z".to_string()),
        ];
        let authorization = sign_request(
            "GET",
            "/",
            "Param1=value1&Param2=value2",
            &headers,
            EMPTY_SHA256,
            &example_signer("service"),
        );
        assert_eq!(
            signature_of(&authorization),
            "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
        );
    }

    #[test]
    fn sigv4_matches_the_aws_form_post_vector() {
        // aws-sig-v4-test-suite: post-x-www-form-urlencoded
        let headers = vec![
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("host".to_string(), "example.amazonaws.com".to_string()),
            ("x-amz-date".to_string(), "20150830T123600Z".to_string()),
        ];
        let payload_hash = sha256_hex(b"Param1=value1");
        let authorization = sign_request(
            "POST",
            "/",
            "",
            &headers,
            &payload_hash,
            &example_signer("service"),
        );
        assert_eq!(
            signature_of(&authorization),
            "ff11897932ad3f4e8b18135d722051e5ac45fc38421b1da7b9d196a0fe09473a"
        );
    }

    #[test]
    fn uri_encoding_keeps_paths_and_escapes_the_rest() {
        assert_eq!(uri_encode("a/b c", false), "a/b%20c");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
        assert_eq!(uri_encode("p1=1&p2=2", true), "p1%3D1%26p2%3D2");
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
    }

    #[test]
    fn list_pages_parse_keys_and_the_next_token() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
            <ListBucketResult>
                <Name>writan</Name>
                <IsTruncated>true</IsTruncated>
                <Contents><Key>items/aaaa/bbbb/1.enc</Key><Size>7</Size></Contents>
                <Contents><Key>items/cccc/dddd/2.enc</Key><Size>9</Size></Contents>
                <NextContinuationToken>1ueGcxLPRx1Tr</NextContinuationToken>
            </ListBucketResult>"#;
        let page = parse_list_page(body).expect("page parses");
        assert_eq!(
            page.keys,
            vec!["items/aaaa/bbbb/1.enc", "items/cccc/dddd/2.enc"]
        );
        assert_eq!(page.next.as_deref(), Some("1ueGcxLPRx1Tr"));
    }

    #[derive(Default)]
    struct FakeTransport {
        objects: Mutex<BTreeMap<String, Vec<u8>>>,
        pages: Mutex<VecDeque<ListPage>>,
    }

    impl S3Transport for FakeTransport {
        fn put_if_absent(&self, path: &str, bytes: &[u8]) -> Result<bool, io::Error> {
            let mut objects = self.objects.lock().expect("fake transport mutex");
            if objects.contains_key(path) {
                return Ok(false);
            }
            objects.insert(path.to_string(), bytes.to_vec());
            Ok(true)
        }

        fn get(&self, path: &str) -> Result<Option<Vec<u8>>, io::Error> {
            Ok(self
                .objects
                .lock()
                .expect("fake transport mutex")
                .get(path)
                .cloned())
        }

        fn list_page(&self, _prefix: &str, _token: Option<&str>) -> Result<ListPage, io::Error> {
            let mut pages = self.pages.lock().expect("fake transport mutex");
            Ok(pages.pop_front().unwrap_or_default())
        }
    }

    #[test]
    fn target_is_create_only_and_accepts_identical_retries() {
        let target = S3Target::new(FakeTransport::default());
        target
            .write("items/a/b/1.enc", b"first")
            .expect("fresh write lands");
        target
            .write("items/a/b/1.enc", b"first")
            .expect("identical retry is a no-op");
        let error = target
            .write("items/a/b/1.enc", b"different")
            .expect_err("different bytes diverge");
        assert!(matches!(error, TargetError::Diverged(path) if path == "items/a/b/1.enc"));
    }

    #[test]
    fn missing_objects_read_as_not_found() {
        let target = S3Target::new(FakeTransport::default());
        let error = target.read("items/a/b/1.enc").expect_err("missing object");
        assert!(matches!(
            error,
            TargetError::Io(error) if error.kind() == io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn listings_are_concatenated_across_pages() {
        let transport = FakeTransport::default();
        {
            let mut pages = transport.pages.lock().expect("fake transport mutex");
            pages.push_back(ListPage {
                keys: vec!["items/a/b/1.enc".to_string()],
                next: Some("token-2".to_string()),
            });
            pages.push_back(ListPage {
                keys: vec!["items/c/d/2.enc".to_string()],
                next: None,
            });
        }
        let target = S3Target::new(transport);
        let keys = target.list().expect("listing works");
        assert_eq!(keys, vec!["items/a/b/1.enc", "items/c/d/2.enc"]);
    }
}
