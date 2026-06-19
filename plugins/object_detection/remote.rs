// SPDX-License-Identifier: GPL-2.0-or-later

use async_trait::async_trait;
use common::{Detections, DynError};
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode, Uri, body::Bytes, header};
use hyper_util::client::legacy::Client;
use plugin::object_detection::{ArcDetector, Detector};
use serde::Deserialize;
use std::{
    num::{NonZeroU16, NonZeroU64},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::runtime::Handle;
use url::Url;

use crate::TokioExecutor;

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
        let rt_handle = self.rt_handle.clone();
        let task_rt_handle = rt_handle.clone();
        let width = self.width;
        let height = self.height;
        let endpoint = self.endpoint.clone();
        let timeout_duration = self.timeout;

        let task = rt_handle.spawn(async move {
            tokio::time::timeout(
                timeout_duration,
                detect_inner(task_rt_handle, width, height, endpoint, data),
            )
            .await
            .map_err(|_| RemoteDetectError::Timeout(timeout_duration))?
        });

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

async fn detect_inner(
    rt_handle: Handle,
    width: NonZeroU16,
    height: NonZeroU16,
    endpoint: Url,
    data: Vec<u8>,
) -> Result<Detections, RemoteDetectError> {
    let uri: Uri = endpoint.as_str().parse()?;
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor(rt_handle)).build(https);

    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header("x-width", width.get().to_string())
        .header("x-height", height.get().to_string())
        .header("x-format", "rgb24")
        .body(Full::new(Bytes::from(data)))?;

    let response = client.request(request).await?;
    let status = response.status();
    let body = response.into_body().collect().await?.to_bytes();
    if status != StatusCode::OK {
        return Err(RemoteDetectError::Status(
            status,
            String::from_utf8_lossy(&body).into(),
        ));
    }

    let response: RemoteDetectResponse = serde_json::from_slice(&body)?;
    Ok(response.detections)
}

#[derive(Debug, Deserialize)]
struct RemoteDetectResponse {
    detections: Detections,
}

#[derive(Debug, Error)]
enum RemoteDetectError {
    #[error("parse uri: {0}")]
    ParseUri(#[from] hyper::http::uri::InvalidUri),

    #[error("build request: {0}")]
    BuildRequest(#[from] hyper::http::Error),

    #[error("request: {0}")]
    Request(#[from] hyper_util::client::legacy::Error),

    #[error("collect response: {0}")]
    Collect(#[from] hyper::Error),

    #[error("remote detector returned HTTP {0}: {1}")]
    Status(StatusCode, String),

    #[error("decode response: {0}")]
    DecodeResponse(#[from] serde_json::Error),

    #[error("remote detector timed out after {0:?}")]
    Timeout(Duration),

    #[error("remote detector task: {0}")]
    Join(#[from] tokio::task::JoinError),
}