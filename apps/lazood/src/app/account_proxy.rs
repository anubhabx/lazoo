//! A session-scoped, authenticated loopback proxy. Streams are never buffered or replayed.
use std::{
    convert::Infallible,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::{BodyExt, Full, Limited, StreamBody, combinators::BoxBody};
use hyper::{
    Request, Response, StatusCode,
    body::{Frame, Incoming},
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use lazoo_protocol::{AccountApiProtocol, ProviderAccountRecord};
use lazoo_runtime::unprotect_for_current_user;
use lazoo_store::Store;
use tokio::{
    net::TcpListener,
    sync::{Mutex, Semaphore},
    task::{JoinHandle, JoinSet},
};
use uuid::Uuid;

use super::{
    RpcError,
    accounts::{account_failure, validate_endpoint},
};

type ProxyBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;
const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
#[path = "account_proxy_tests.rs"]
mod tests;

pub(super) struct AccountProxy {
    endpoint: String,
    token: String,
    listener: JoinHandle<()>,
}

impl Drop for AccountProxy {
    fn drop(&mut self) {
        self.listener.abort();
    }
}

struct ProxyState {
    store: Store,
    protocol: AccountApiProtocol,
    session_id: Uuid,
    token: String,
    client: reqwest::Client,
    owner: Mutex<Owner>,
    connections: Arc<Semaphore>,
}

struct Owner {
    account_id: Uuid,
    accepted: bool,
}

impl AccountProxy {
    pub(super) async fn start(
        store: Store,
        session_id: Uuid,
        account: &ProviderAccountRecord,
        resumed: bool,
    ) -> Result<Self, RpcError> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| account_failure("could not bind account proxy"))?;
        let port = listener
            .local_addr()
            .map_err(|_| account_failure("could not read account proxy address"))?
            .port();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let protocol = account
            .spec
            .api_protocol
            .ok_or_else(|| account_failure("account does not support API proxying"))?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(90))
            .timeout(Duration::from_secs(600))
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| account_failure("could not initialize HTTPS account transport"))?;
        let upstream_id = store
            .provider_account_binding(session_id)
            .map_err(|_| account_failure("account ownership is unavailable"))?
            .and_then(|binding| binding.upstream_account_id)
            .unwrap_or(account.spec.id);
        let state = Arc::new(ProxyState {
            store,
            protocol,
            session_id,
            token: token.clone(),
            client,
            owner: Mutex::new(Owner {
                account_id: upstream_id,
                accepted: resumed,
            }),
            connections: Arc::new(Semaphore::new(16)),
        });
        let listener = tokio::spawn(async move {
            let mut workers = JoinSet::new();
            loop {
                tokio::select! {
                    next = listener.accept() => {
                        let Ok((stream, address)) = next else { break; };
                        if !address.ip().is_loopback() { continue; }
                        let Ok(permit) = Arc::clone(&state.connections).try_acquire_owned() else { continue; };
                        let state = Arc::clone(&state);
                        workers.spawn(async move {
                            let _permit = permit;
                            let service = service_fn(move |request| {
                                let state = Arc::clone(&state);
                                async move { Ok::<_, Infallible>(state.handle(request).await) }
                            });
                            let _ = hyper::server::conn::http1::Builder::new()
                                .serve_connection(TokioIo::new(stream), service).await;
                        });
                    }
                    _ = workers.join_next(), if !workers.is_empty() => {}
                }
            }
        });
        Ok(Self {
            endpoint: format!("http://127.0.0.1:{port}"),
            token,
            listener,
        })
    }

    pub(super) fn endpoint(&self, _protocol: AccountApiProtocol) -> String {
        format!("{}/v1", self.endpoint)
    }
    pub(super) fn token(&self) -> &str {
        &self.token
    }
}

impl ProxyState {
    #[allow(clippy::too_many_lines)] // The admission lock and no-replay boundary stay visibly linear.
    async fn handle(&self, request: Request<Incoming>) -> Response<ProxyBody> {
        let authorized = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == format!("Bearer {}", self.token))
            || request
                .headers()
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == self.token);
        if !authorized {
            return failure(StatusCode::UNAUTHORIZED, "invalid local routing credential");
        }
        let route = match request.uri().path() {
            "/v1/responses" | "/responses" if self.protocol == AccountApiProtocol::Responses => {
                "responses"
            }
            "/v1/responses/compact" | "/responses/compact"
                if self.protocol == AccountApiProtocol::Responses =>
            {
                "responses/compact"
            }
            "/v1/messages" | "/messages" if self.protocol == AccountApiProtocol::Anthropic => {
                "messages"
            }
            "/v1/messages/count_tokens" | "/messages/count_tokens"
                if self.protocol == AccountApiProtocol::Anthropic =>
            {
                "messages/count_tokens"
            }
            "/v1/models" | "/models" => "models",
            _ => return failure(StatusCode::NOT_FOUND, "unsupported account proxy route"),
        };
        let expected_method = if route == "models" {
            hyper::Method::GET
        } else {
            hyper::Method::POST
        };
        let query = request.uri().query().map(str::to_owned);
        let supported_query = query.is_none()
            || (self.protocol == AccountApiProtocol::Anthropic
                && matches!(route, "messages" | "messages/count_tokens")
                && query.as_deref() == Some("beta=true"));
        if request.method() != expected_method || !supported_query {
            return failure(
                StatusCode::METHOD_NOT_ALLOWED,
                "unsupported account proxy request",
            );
        }
        let headers = request.headers().clone();
        let body = match tokio::time::timeout(
            Duration::from_secs(30),
            Limited::new(request.into_body(), MAX_REQUEST_BYTES).collect(),
        )
        .await
        {
            Ok(Ok(body)) => body.to_bytes(),
            _ => {
                return failure(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body unavailable or exceeds routing limit",
                );
            }
        };
        let payload: serde_json::Value = if body.is_empty() && route == "models" {
            serde_json::json!({})
        } else {
            match serde_json::from_slice(&body) {
                Ok(value) => value,
                Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid request JSON"),
            }
        };
        let model = payload.get("model").and_then(serde_json::Value::as_str);
        // Hold the ownership lock through admission. After acceptance, later requests reuse this
        // identity; an incomplete stream is never retried on a different account.
        let mut owner = self.owner.lock().await;
        let mut excluded = Vec::new();
        for attempt in 0..2 {
            let account = match self.store.provider_account(owner.account_id) {
                Ok(account)
                    if account.spec.enabled
                        && !account.deleted
                        && account.cooldown_until <= chrono::Utc::now().timestamp() =>
                {
                    account
                }
                _ => {
                    return failure(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "the session's account is unavailable",
                    );
                }
            };
            if route != "models" && !account.spec.accepts_model(model) {
                return failure(
                    StatusCode::FORBIDDEN,
                    "model is unavailable for this account",
                );
            }
            let default_endpoint = match self.protocol {
                AccountApiProtocol::Anthropic => "https://api.anthropic.com/v1",
                AccountApiProtocol::Responses => "https://api.openai.com/v1",
            };
            let base = account
                .spec
                .base_url
                .as_deref()
                .unwrap_or(default_endpoint)
                .trim_end_matches('/');
            let base = if self.protocol == AccountApiProtocol::Anthropic && !base.ends_with("/v1") {
                format!("{base}/v1")
            } else {
                base.to_owned()
            };
            let endpoint = format!("{base}/{route}");
            let Ok(mut endpoint) = validate_endpoint(&endpoint) else {
                return failure(StatusCode::BAD_GATEWAY, "invalid account endpoint");
            };
            endpoint.set_query(query.as_deref());
            let Some(secret) = self
                .store
                .provider_account_secret(account.spec.id)
                .ok()
                .flatten()
                .and_then(|cipher| unprotect_for_current_user(&cipher).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok())
            else {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "account credential cannot be opened",
                );
            };
            let mut upstream = self
                .client
                .request(expected_method.clone(), endpoint)
                .body(body.clone());
            for name in [
                "content-type",
                "accept",
                "anthropic-version",
                "anthropic-beta",
                "openai-beta",
            ] {
                if let Some(value) = headers.get(name) {
                    upstream = upstream.header(name, value);
                }
            }
            upstream = match self.protocol {
                AccountApiProtocol::Anthropic => upstream.header("x-api-key", secret),
                AccountApiProtocol::Responses => upstream.bearer_auth(secret),
            };
            let started = Instant::now();
            let Ok(response) = upstream.send().await else {
                let _ = self.store.observe_provider_account(
                    account.spec.id,
                    502,
                    elapsed_millis(started),
                    5,
                );
                // A transport error cannot prove the server did not receive the request.
                return failure(
                    StatusCode::BAD_GATEWAY,
                    "upstream transport failed; request was not replayed",
                );
            };
            let status = response.status();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
                .unwrap_or(60);
            let _ = self.store.observe_provider_account(
                account.spec.id,
                status.as_u16(),
                elapsed_millis(started),
                retry_after,
            );
            let continuation = contains_account_state(&payload) || route == "responses/compact";
            if matches!(status.as_u16(), 401 | 403 | 429)
                && !owner.accepted
                && !continuation
                && attempt == 0
            {
                excluded.push(account.spec.id);
                if let Ok(next) = self
                    .store
                    .select_pool_account(&account.spec, model, &excluded)
                    && self
                        .store
                        .rebind_provider_api_account(self.session_id, account.spec.id, next.spec.id)
                        .is_ok()
                {
                    owner.account_id = next.spec.id;
                    continue;
                }
            }
            if status.is_success() && route != "models" && route != "messages/count_tokens" {
                owner.accepted = true;
            }
            drop(owner);
            let mut outgoing = Response::builder().status(status);
            for name in [
                "content-type",
                "cache-control",
                "retry-after",
                "request-id",
                "x-request-id",
                "openai-processing-ms",
            ] {
                if let Some(value) = response.headers().get(name) {
                    outgoing = outgoing.header(name, value);
                }
            }
            let stream = response
                .bytes_stream()
                .map_ok(Frame::data)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>);
            return outgoing
                .body(BodyExt::boxed(StreamBody::new(stream)))
                .unwrap_or_else(|_| failure(StatusCode::BAD_GATEWAY, "invalid upstream response"));
        }
        failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "no account accepted the request",
        )
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
fn contains_account_state(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => fields.iter().any(|(name, value)| {
            (matches!(
                name.as_str(),
                "previous_response_id"
                    | "conversation"
                    | "file_id"
                    | "response_id"
                    | "encrypted_content"
                    | "signature"
                    | "container_id"
                    | "vector_store_ids"
                    | "upload_id"
            ) && !value.is_null())
                || contains_account_state(value)
        }),
        serde_json::Value::Array(values) => values.iter().any(contains_account_state),
        _ => false,
    }
}
fn failure(status: StatusCode, message: &str) -> Response<ProxyBody> {
    let bytes = Bytes::from(
        serde_json::json!({"error":{"message":message,"type":"account_routing_error"}}).to_string(),
    );
    let body = Full::new(bytes)
        .map_err(|error: Infallible| match error {})
        .boxed();
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body)
        .expect("fixed response headers")
}
