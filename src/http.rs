use std::{
    net::{IpAddr, Ipv4Addr},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use hyper::{
    HeaderMap,
    header::{
        ACCEPT_ENCODING, ACCEPT_RANGES, CACHE_CONTROL, CONNECTION, CONTENT_ENCODING,
        CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, EXPIRES, HOST, HeaderName, HeaderValue,
        LAST_MODIFIED, LOCATION, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, SET_COOKIE,
        TRANSFER_ENCODING, UPGRADE, VIA,
    },
};
use reqwest::{Client, Method, StatusCode, redirect};

use crate::chunk::Chunk;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub content_length: Option<u64>,
    pub accepts_ranges: bool,
    pub final_url: String,
}

#[derive(Debug)]
pub struct TransferResult {
    pub bytes: Bytes,
    pub duration: Duration,
    pub status: StatusCode,
    pub headers: HeaderMap,
}

#[derive(Debug)]
pub struct ChunkResult {
    pub bytes: Bytes,
    pub duration: Duration,
    pub headers: HeaderMap,
}

pub fn client_for(ip: Ipv4Addr) -> Result<Client> {
    Client::builder()
        .local_address(IpAddr::V4(ip))
        .connect_timeout(CONNECT_TIMEOUT)
        .no_proxy()
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.stop();
            }

            if attempt.url().scheme() == "http" {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .context("failed to build reqwest client")
}

pub async fn head(client: &Client, url: &str, request_headers: &HeaderMap) -> Result<ServerInfo> {
    let headers = upstream_headers(request_headers, true);
    let started = Instant::now();
    let response = client
        .head(url)
        .headers(headers)
        .send()
        .await
        .with_context(|| format!("HEAD failed for {url}"))?;

    if !response.status().is_success() {
        bail!("HEAD failed: {}", response.status());
    }

    let headers = response.headers();
    let content_length = parse_content_length(headers);
    let accepts_ranges = headers
        .get(ACCEPT_RANGES)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("bytes"));

    let _duration = started.elapsed();

    Ok(ServerInfo {
        content_length,
        accepts_ranges,
        final_url: response.url().to_string(),
    })
}

pub async fn get_range(
    client: &Client,
    url: &str,
    request_headers: &HeaderMap,
    chunk: Chunk,
    total_size: u64,
    on_rx: impl FnMut(u64),
) -> Result<ChunkResult> {
    let mut headers = upstream_headers(request_headers, true);
    headers.insert(
        HeaderName::from_static("range"),
        HeaderValue::from_str(&format!("bytes={}-{}", chunk.start, chunk.end))
            .context("failed to build Range header")?,
    );

    let started = Instant::now();
    let response = client
        .get(url)
        .headers(headers)
        .send()
        .await
        .with_context(|| format!("range GET failed for bytes {}-{}", chunk.start, chunk.end))?;

    if response.status() != StatusCode::PARTIAL_CONTENT {
        bail!(
            "unexpected range status for bytes {}-{}: {}",
            chunk.start,
            chunk.end,
            response.status()
        );
    }

    validate_content_range(response.headers(), chunk, total_size)?;
    let headers = response.headers().clone();
    let bytes = read_response_body(
        response,
        on_rx,
        &format!("failed to read range body {}-{}", chunk.start, chunk.end),
    )
    .await?;
    let duration = started.elapsed();

    if bytes.len() as u64 != chunk.len() {
        bail!(
            "range body length mismatch for bytes {}-{}: expected {}, got {}",
            chunk.start,
            chunk.end,
            chunk.len(),
            bytes.len()
        );
    }

    Ok(ChunkResult {
        bytes,
        duration,
        headers,
    })
}

pub async fn get_full(
    client: &Client,
    url: &str,
    request_headers: &HeaderMap,
    on_rx: impl FnMut(u64),
) -> Result<TransferResult> {
    request_with_body(
        client,
        Method::GET,
        url,
        request_headers,
        Bytes::new(),
        on_rx,
    )
    .await
}

pub async fn request_with_body(
    client: &Client,
    method: Method,
    url: &str,
    request_headers: &HeaderMap,
    body: Bytes,
    on_rx: impl FnMut(u64),
) -> Result<TransferResult> {
    let headers = upstream_headers(request_headers, false);
    let started = Instant::now();
    let mut builder = client.request(method, url).headers(headers);

    if !body.is_empty() {
        builder = builder.body(body);
    }

    let response = builder
        .send()
        .await
        .with_context(|| format!("upstream request failed for {url}"))?;

    let status = response.status();
    let headers = response.headers().clone();
    let bytes = read_response_body(
        response,
        on_rx,
        &format!("failed to read upstream body for {url}"),
    )
    .await?;

    Ok(TransferResult {
        bytes,
        duration: started.elapsed(),
        status,
        headers,
    })
}

async fn read_response_body(
    response: reqwest::Response,
    mut on_rx: impl FnMut(u64),
    context: &str,
) -> Result<Bytes> {
    let mut body = BytesMut::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| context.to_string())?;
        on_rx(chunk.len() as u64);
        body.extend_from_slice(&chunk);
    }

    Ok(body.freeze())
}

pub fn upstream_headers(request_headers: &HeaderMap, force_identity: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();

    for (name, value) in request_headers {
        if should_skip_request_header(name) {
            continue;
        }
        headers.append(name.clone(), value.clone());
    }

    if force_identity {
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    }

    headers
}

pub fn response_headers(source: &HeaderMap, content_length: u64) -> HeaderMap {
    let mut headers = HeaderMap::new();

    for name in [
        CONTENT_TYPE,
        CONTENT_ENCODING,
        ETAG,
        LAST_MODIFIED,
        CACHE_CONTROL,
        EXPIRES,
        ACCEPT_RANGES,
        SET_COOKIE,
        LOCATION,
        VIA,
    ] {
        for value in source.get_all(&name) {
            headers.append(name.clone(), value.clone());
        }
    }

    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&content_length.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );
    headers
}

fn parse_content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn validate_content_range(headers: &HeaderMap, chunk: Chunk, total_size: u64) -> Result<()> {
    let content_range = headers
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .context("missing Content-Range header in 206 response")?;
    let expected = format!("bytes {}-{}/{}", chunk.start, chunk.end, total_size);

    if content_range != expected {
        bail!("unexpected Content-Range: expected {expected}, got {content_range}");
    }

    Ok(())
}

fn should_skip_request_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str().to_ascii_lowercase().as_str(),
        "connection"
            | "proxy-connection"
            | "transfer-encoding"
            | "keep-alive"
            | "upgrade"
            | "te"
            | "trailer"
            | "host"
            | "content-length"
    ) || *name == PROXY_AUTHORIZATION
        || *name == PROXY_AUTHENTICATE
        || *name == HOST
        || *name == CONNECTION
        || *name == TRANSFER_ENCODING
        || *name == UPGRADE
}
