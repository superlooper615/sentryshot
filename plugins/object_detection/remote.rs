// SPDX-License-Identifier: GPL-2.0-or-later

use async_trait::async_trait;
use common::{Detections, DynError};
use plugin::object_detection::{ArcDetector, Detector};
use serde::Deserialize;
use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    num::{NonZeroU16, NonZeroU64},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::runtime::Handle;
use url::Url;

pub(crate) struct RemoteDetector {
    rt_handle: Handle,
    width: NonZeroU16,
    height: NonZeroU16,
    endpoint: Url,
    timeout: Duration,
}

impl RemoteDetector {
    pub(crate) fn new(
        rt_handle: Handle,
        width: NonZeroU16,
        height: NonZeroU16,
        endpoint: Url,
        timeout_ms: NonZeroU64,
    ) -> ArcDetector {
        Arc::new(Self {
            rt_handle,
            width,
            height,
            endpoint,
            timeout: Duration::from_millis(timeout_ms.get()),
        })
    }
}

#[async_trait]
impl Detector for RemoteDetector {
    async fn detect(&self, data: Vec<u8>) -> Result<Option<Detections>, DynError> {
        let width = self.width;
        let height = self.height;
        let endpoint = self.endpoint.clone();
        let timeout = self.timeout;

        let task = self
            .rt_handle
            .spawn_blocking(move || detect_blocking(width, height, endpoint, timeout, data));

        let detections = task.await.map_err(RemoteDetectError::Join)??;
        Ok(Some(detections))
    }

    fn width(&self) -> NonZeroU16 {
        self.width
    }

    fn height(&self) -> NonZeroU16 {
        self.height
    }
}

fn detect_blocking(
    width: NonZeroU16,
    height: NonZeroU16,
    endpoint: Url,
    timeout: Duration,
    data: Vec<u8>,
) -> Result<Detections, RemoteDetectError> {
    if endpoint.scheme() != "http" {
        return Err(RemoteDetectError::UnsupportedScheme(endpoint.scheme().to_owned()));
    }

    let host = endpoint.host_str().ok_or(RemoteDetectError::MissingHost)?;
    let port = endpoint.port_or_known_default().ok_or(RemoteDetectError::MissingPort)?;
    let mut addrs = (host, port).to_socket_addrs()?;
    let addr = addrs.next().ok_or(RemoteDetectError::ResolveEmpty)?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let mut path = endpoint.path().to_owned();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(query) = endpoint.query() {
        path.push('?');
        path.push_str(query);
    }

    let host_header = if endpoint.port().is_some() {
        format!("{host}:{port}")
    } else {
        host.to_owned()
    };
    let header = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_header}\r\nContent-Type: application/octet-stream\r\nx-width: {}\r\nx-height: {}\r\nx-format: rgb24\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        width.get(),
        height.get(),
        data.len(),
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&data)?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(RemoteDetectError::BadResponse("missing header terminator"))?;
    let (headers, body) = response.split_at(header_end + 4);
    let headers = std::str::from_utf8(headers)
        .map_err(|_| RemoteDetectError::BadResponse("headers are not utf8"))?;
    let status = parse_status(headers)?;
    if status != 200 {
        return Err(RemoteDetectError::Status(status, String::from_utf8_lossy(body).into()));
    }

    let response: RemoteDetectResponse = serde_json::from_slice(body)?;
    Ok(response.detections)
}

fn parse_status(headers: &str) -> Result<u16, RemoteDetectError> {
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or(RemoteDetectError::BadResponse("missing status"))?;
    status
        .parse()
        .map_err(|_| RemoteDetectError::BadResponse("bad status"))
}

#[derive(Debug, Deserialize)]
struct RemoteDetectResponse {
    detections: Detections,
}

#[derive(Debug, Error)]
enum RemoteDetectError {
    #[error("unsupported URL scheme: {0}")]
    UnsupportedScheme(String),

    #[error("endpoint is missing a host")]
    MissingHost,

    #[error("endpoint is missing a port")]
    MissingPort,

    #[error("resolve endpoint: no addresses returned")]
    ResolveEmpty,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("bad response: {0}")]
    BadResponse(&'static str),

    #[error("remote detector returned HTTP {0}: {1}")]
    Status(u16, String),

    #[error("decode response: {0}")]
    DecodeResponse(#[from] serde_json::Error),

    #[error("remote detector task: {0}")]
    Join(#[from] tokio::task::JoinError),
}