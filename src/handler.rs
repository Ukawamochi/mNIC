use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,//時間を扱う(時計ではなく、Nmsみたいな)
};

use anyhow::{Context, Result, anyhow, bail};
use bytes::{Bytes, BytesMut};//Bytesは読み取り専用のバイト列、BytesMutは書き換え可能
use http_body_util::{BodyExt, Full};//httpのBodyを扱う。BodyExtはメゾットを追加する
use hyper::{//hyperはTCPで受け取ったバイト列をhttpに対応したstructに変換する
    Method,//HTTPメゾットの型
    Request,//HTTPリクエストの要素(メゾット、URL、ヘッダなど)の型
    Response,//HTTPレスポンスの要素(Status,ヘッダ、ボディなど)の型
    StatusCode,//HTTPステータスコードの型
    Uri,//リクエスト先のURIを扱う型
    body::Incoming,//不完全なボディの型。すべて集まるとFullになる
    header::CONTENT_TYPE,//httpヘッダを表す型
    upgrade,//プロキシがパケットを解析せず中継だけする
};
use hyper_util::rt::TokioIo;//tokioとhyperの接続
use reqwest::Method as ReqwestMethod;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpSocket, lookup_host},//
    time::timeout,//非同期処理に時間制限をつける。これにDurationで作った時間量を入れる
};

//crateは自プロジェクト
use crate::{
    chunk,
    config::NicConfig,
    http::{self, ServerInfo},
    proxy::{ConnectionContext, ProxyState},
    stats::SharedStats,
};

type RespBody = Full<Bytes>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

//ルーティング(GET、CONNECT,その他の3つで)
pub async fn route(
    req: Request<Incoming>,
    state: ProxyState,
    connection: ConnectionContext,
) -> Response<RespBody> {
    if req.method() == Method::CONNECT {
        handle_connect(req, state, connection).await
    } else if req.method() == Method::GET {
        handle_get(req, state, connection).await
    } else {
        handle_passthrough(req, state, connection).await
    }
}
//httpsのGETのとき(range_get()とfull_get()はこの関数が呼び出す)
async fn handle_get(
    req: Request<Incoming>,
    state: ProxyState,
    connection: ConnectionContext,
) -> Response<RespBody> {
    let url = match absolute_http_url(req.uri()) {
        Ok(url) => url,
        Err(error) => {
            state
                .stats
                .fail_connection(connection.id, error.to_string());
            return error_response(StatusCode::BAD_REQUEST, &error);
        }
    };

    let request_headers = req.headers().clone();
    let nic = &state.config.nics[connection.nic_index];
    let client = &state.clients[connection.nic_index];
    state.stats.start_connection(
        connection.id,
        connection.nic_index,
        "HTTP GET",
        &url,
        if state.options.range_split_enabled {
            "head"
        } else {
            "full"
        },
    );

    if !state.options.range_split_enabled {
        return handle_full_get(
            &url,
            None,
            None,
            &request_headers,
            nic,
            client,
            &state,
            connection,
        )
        .await;
    }

    let (server_info, fallback_note) = match http::head(client, &url, &request_headers).await {
        Ok(info) => (Some(info), None),
        Err(error) => {
            let note = format!("HEAD failed ({error}), falling back to single NIC");
            state
                .stats
                .record_event(format!("GET #{} {note}", connection.id));
            (None, Some(note))
        }
    };

    if state.options.range_split_enabled
        && let Some(info) = &server_info
        && let Some(size) = info.content_length
        && info.accepts_ranges
        && size >= 2
    {
        return handle_range_get(&url, info, size, &request_headers, &state, connection).await;
    }

    handle_full_get(
        &url,
        server_info.as_ref(),
        fallback_note.as_deref(),
        &request_headers,
        nic,
        client,
        &state,
        connection,
    )
    .await
}

async fn handle_range_get(
    original_url: &str,
    server_info: &ServerInfo,
    size: u64,
    request_headers: &hyper::HeaderMap,
    state: &ProxyState,
    connection: ConnectionContext,
) -> Response<RespBody> {
    let nic1 = &state.config.nics[0];
    let nic2 = &state.config.nics[1];
    let client1 = &state.clients[0];
    let client2 = &state.clients[1];
    let [first_chunk, second_chunk] = chunk::split_in_half(size);
    let url = &server_info.final_url;
    state.stats.start_connection(
        connection.id,
        connection.nic_index,
        "HTTP GET",
        original_url,
        "range",
    );
    state.stats.record_event(format!(
        "GET #{} {original_url} range split started",
        connection.id
    ));
    let stats_first = state.stats.clone();
    let stats_second = state.stats.clone();
    let first = http::get_range(
        client1,
        url,
        request_headers,
        first_chunk,
        size,
        move |bytes| stats_first.add_rx(connection.id, 0, bytes),
    );
    let second = http::get_range(
        client2,
        url,
        request_headers,
        second_chunk,
        size,
        move |bytes| stats_second.add_rx(connection.id, 1, bytes),
    );
    let (first, second) = tokio::join!(first, second);

    let (first, second) = match (first, second) {
        (Ok(first), Ok(second)) => (first, second),
        (Err(error), _) => {
            state.stats.fail_connection(
                connection.id,
                format!("range GET via NIC {} failed: {error}", nic1.ip),
            );
            return error_response(StatusCode::BAD_GATEWAY, &error);
        }
        (_, Err(error)) => {
            state.stats.fail_connection(
                connection.id,
                format!("range GET via NIC {} failed: {error}", nic2.ip),
            );
            return error_response(StatusCode::BAD_GATEWAY, &error);
        }
    };
    //不完全だったBodyを結合する
    let mut body = BytesMut::with_capacity(size as usize);
    body.extend_from_slice(&first.bytes);
    body.extend_from_slice(&second.bytes);
    let headers = http::response_headers(&first.headers, size);

    state.stats.record_event(format!(
        "GET #{} range split completed: NIC[0] {}-{} in {:.2}s, NIC[1] {}-{} in {:.2}s",
        connection.id,
        first_chunk.start,
        first_chunk.end,
        first.duration.as_secs_f64(),
        second_chunk.start,
        second_chunk.end,
        second.duration.as_secs_f64()
    ));
    state.stats.finish_connection(connection.id, "closed");

    response_with_headers(StatusCode::OK, headers, body.freeze())
}

async fn handle_full_get(
    url: &str,
    server_info: Option<&ServerInfo>,
    fallback_note: Option<&str>,
    request_headers: &hyper::HeaderMap,
    nic: &NicConfig,
    client: &reqwest::Client,
    state: &ProxyState,
    connection: ConnectionContext,
) -> Response<RespBody> {
    state.stats.set_state(connection.id, "full");
    let stats = state.stats.clone();
    match http::get_full(
        client,
        server_info.map_or(url, |info| &info.final_url),
        request_headers,
        move |bytes| stats.add_rx(connection.id, connection.nic_index, bytes),
    )
    .await
    {
        Ok(result) => {
            let headers = http::response_headers(&result.headers, result.bytes.len() as u64);
            let range_label = if server_info.is_some_and(|info| info.accepts_ranges) {
                "yes (no split)"
            } else {
                "no"
            };
            state.stats.record_event(format!(
                "GET #{} completed via NIC {} size {:?} range {} note {:?} in {:.2}s",
                connection.id,
                nic.ip,
                server_info.and_then(|info| info.content_length),
                range_label,
                fallback_note,
                result.duration.as_secs_f64()
            ));
            state.stats.finish_connection(connection.id, "closed");
            response_with_headers(status_from_reqwest(result.status), headers, result.bytes)
        }
        Err(error) => {
            state
                .stats
                .fail_connection(connection.id, error.to_string());
            error_response(StatusCode::BAD_GATEWAY, &error)
        }
    }
}

//CONNECT
async fn handle_connect(
    req: Request<Incoming>,
    state: ProxyState,
    connection: ConnectionContext,
) -> Response<RespBody> {
    let target = req.uri().authority().map(ToString::to_string);
    let Some(target) = target else {
        let error = anyhow!("CONNECT request missing authority");
        state
            .stats
            .fail_connection(connection.id, error.to_string());
        return error_response(StatusCode::BAD_REQUEST, &error);
    };

    let nic = &state.config.nics[connection.nic_index];
    state.stats.start_connection(
        connection.id,
        connection.nic_index,
        "CONNECT",
        &target,
        "connecting",
    );
    let server = match connect_to_target(&target, nic.ip).await {
        Ok(server) => server,
        Err(error) => {
            state
                .stats
                .fail_connection(connection.id, error.to_string());
            return error_response(StatusCode::BAD_GATEWAY, &error);
        }
    };

    state.stats.start_connection(
        connection.id,
        connection.nic_index,
        "CONNECT",
        &target,
        "open",
    );
    let stats = state.stats.clone();
    tokio::spawn(async move {
        match upgrade::on(req).await {
            Ok(upgraded) => {
                let browser = TokioIo::new(upgraded);
                if let Err(error) = relay_tunnel(browser, server, stats.clone(), connection).await {
                    stats.fail_connection(connection.id, error.to_string());
                } else {
                    stats.finish_connection(connection.id, "closed");
                }
            }
            Err(error) => stats.fail_connection(connection.id, format!("upgrade failed: {error}")),
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .body(Full::new(Bytes::new()))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

//CONNECTとGET以外
async fn handle_passthrough(
    req: Request<Incoming>,
    state: ProxyState,
    connection: ConnectionContext,
) -> Response<RespBody> {
    let method = req.method().clone();
    let method_name = method.as_str().to_string();
    let url = match absolute_http_url(req.uri()) {
        Ok(url) => url,
        Err(error) => {
            state
                .stats
                .fail_connection(connection.id, error.to_string());
            return error_response(StatusCode::BAD_REQUEST, &error);
        }
    };
    state.stats.start_connection(
        connection.id,
        connection.nic_index,
        method_name.clone(),
        &url,
        "request",
    );
    let headers = req.headers().clone();
    let body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            let error = anyhow!("failed to read request body: {error}");
            state
                .stats
                .fail_connection(connection.id, error.to_string());
            return error_response(StatusCode::BAD_REQUEST, &error);
        }
    };
    let reqwest_method = match ReqwestMethod::from_bytes(method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(error) => {
            let error = anyhow!("unsupported method {method}: {error}");
            state
                .stats
                .fail_connection(connection.id, error.to_string());
            return error_response(StatusCode::BAD_REQUEST, &error);
        }
    };

    let nic = &state.config.nics[connection.nic_index];
    let client = &state.clients[connection.nic_index];
    state
        .stats
        .add_tx(connection.id, connection.nic_index, body.len() as u64);
    let stats = state.stats.clone();
    match http::request_with_body(client, reqwest_method, &url, &headers, body, move |bytes| {
        stats.add_rx(connection.id, connection.nic_index, bytes)
    })
    .await
    {
        Ok(result) => {
            state.stats.record_event(format!(
                "{} #{} completed via NIC {} in {:.2}s",
                method_name,
                connection.id,
                nic.ip,
                result.duration.as_secs_f64()
            ));
            state.stats.finish_connection(connection.id, "closed");
            let headers = http::response_headers(&result.headers, result.bytes.len() as u64);
            response_with_headers(status_from_reqwest(result.status), headers, result.bytes)
        }
        Err(error) => {
            state
                .stats
                .fail_connection(connection.id, error.to_string());
            error_response(StatusCode::BAD_GATEWAY, &error)
        }
    }
}

//中継する
async fn relay_tunnel(
    browser: TokioIo<hyper::upgrade::Upgraded>,
    server: tokio::net::TcpStream,
    stats: SharedStats,
    connection: ConnectionContext,
) -> Result<()> {
    let (browser_read, browser_write) = tokio::io::split(browser);
    let (server_read, server_write) = tokio::io::split(server);
    let upstream_stats = stats.clone();
    let downstream_stats = stats;

    let upstream = copy_metered(browser_read, server_write, move |bytes| {
        upstream_stats.add_tx(connection.id, connection.nic_index, bytes);
    });
    let downstream = copy_metered(server_read, browser_write, move |bytes| {
        downstream_stats.add_rx(connection.id, connection.nic_index, bytes);
    });

    let (upstream, downstream) = tokio::join!(upstream, downstream);
    upstream?;
    downstream?;
    Ok(())
}

async fn copy_metered<R, W>(
    mut reader: R,
    mut writer: W,
    mut on_bytes: impl FnMut(u64),
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = [0_u8; 16 * 1024];

    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .context("tunnel read failed")?;
        if read == 0 {
            writer.shutdown().await.context("tunnel shutdown failed")?;
            return Ok(());
        }
        writer
            .write_all(&buffer[..read])
            .await
            .context("tunnel write failed")?;
        on_bytes(read as u64);
    }
}

async fn connect_to_target(target: &str, local_ip: Ipv4Addr) -> Result<tokio::net::TcpStream> {
    let (host, port) = split_host_port(target)?;
    let mut addrs = lookup_host((host.as_str(), port))
        .await
        .with_context(|| format!("DNS lookup failed for {target}"))?;
    let target_addr = addrs
        .find(|addr| addr.is_ipv4())
        .with_context(|| format!("no IPv4 address found for {target}"))?;

    let socket = TcpSocket::new_v4().context("failed to create IPv4 socket")?;
    socket
        .bind(SocketAddr::new(IpAddr::V4(local_ip), 0))
        .with_context(|| format!("failed to bind local NIC address {local_ip}"))?;

    timeout(CONNECT_TIMEOUT, socket.connect(target_addr))
        .await
        .with_context(|| format!("CONNECT timed out for {target}"))?
        .with_context(|| format!("failed to connect to {target_addr}"))
}

fn split_host_port(target: &str) -> Result<(String, u16)> {
    let uri = format!("http://{target}")
        .parse::<Uri>()
        .with_context(|| format!("invalid CONNECT target: {target}"))?;
    let authority = uri
        .authority()
        .with_context(|| format!("CONNECT target missing authority: {target}"))?;
    Ok((
        authority.host().to_string(),
        authority.port_u16().unwrap_or(443),
    ))
}

fn absolute_http_url(uri: &Uri) -> Result<String> {
    if uri.scheme_str() != Some("http") {
        bail!("only absolute HTTP URLs are supported: {uri}");
    }
    if uri.authority().is_none() {
        bail!("request URI is missing authority: {uri}");
    }
    Ok(uri.to_string())
}

fn response_with_headers(
    status: StatusCode,
    headers: hyper::HeaderMap,
    body: Bytes,
) -> Response<RespBody> {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        if let Some(name) = name {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(Full::new(body))
        .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, &anyhow!("invalid response")))
}

fn error_response(status: StatusCode, error: &anyhow::Error) -> Response<RespBody> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(format!("{status}: {error}\n"))))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

fn status_from_reqwest(status: reqwest::StatusCode) -> StatusCode {
    StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY)
}
