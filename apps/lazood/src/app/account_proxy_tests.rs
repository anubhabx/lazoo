use super::*;
use lazoo_protocol::{
    AccountCredentialKind, AccountRoutingMode, ProviderAccountPolicy, ProviderAccountSpec,
};
use lazoo_runtime::protect_for_current_user;

struct Upstream {
    url: String,
    requests: Arc<Mutex<Vec<(String, Bytes, String)>>>,
    worker: JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

async fn upstream(status: StatusCode, delayed_stream: bool) -> Upstream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let worker = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                incoming = listener.accept() => {
                    let Ok((socket, _)) = incoming else { break; };
                    let captured = Arc::clone(&captured);
                    connections.spawn(async move {
                        let service = service_fn(move |request: Request<Incoming>| {
                            let captured = Arc::clone(&captured);
                            async move {
                                let auth = request.headers().get("authorization").or_else(|| request.headers().get("x-api-key")).and_then(|value| value.to_str().ok()).unwrap_or("").to_owned();
                                let path = request.uri().to_string();
                                let body = request.into_body().collect().await.unwrap().to_bytes();
                                captured.lock().await.push((auth, body, path));
                                let body: ProxyBody = if delayed_stream {
                                    let stream = futures_util::stream::unfold(0, |index| async move {
                                        match index {
                                            0 => Some((Ok::<_, Box<dyn std::error::Error + Send + Sync>>(Frame::data(Bytes::from_static(b"data: first\n\n"))), 1)),
                                            1 => { tokio::time::sleep(Duration::from_millis(250)).await; Some((Ok(Frame::data(Bytes::from_static(b"data: last\n\n"))), 2)) }
                                            _ => None,
                                        }
                                    });
                                    BodyExt::boxed(StreamBody::new(stream))
                                } else {
                                    Full::new(Bytes::from_static(b"{\"ok\":true}")).map_err(|error: Infallible| match error {}).boxed()
                                };
                                Ok::<_, Infallible>(Response::builder().status(status).header("content-type", if delayed_stream {"text/event-stream"} else {"application/json"}).header("retry-after", "60").body(body).unwrap())
                            }
                        });
                        let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service).await;
                    });
                }
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        }
    });
    Upstream {
        url,
        requests,
        worker,
    }
}

fn add_key(
    store: &Store,
    directory: &std::path::Path,
    id: u128,
    endpoint: &str,
) -> ProviderAccountRecord {
    let spec = ProviderAccountSpec {
        id: Uuid::from_u128(id),
        provider: "codex".into(),
        label: format!("Key {id}"),
        credential_kind: AccountCredentialKind::ApiKey,
        profile_directory: directory
            .join(id.to_string())
            .to_string_lossy()
            .into_owned(),
        credential_environment: Some("OPENAI_API_KEY".into()),
        base_url: Some(endpoint.into()),
        api_protocol: Some(AccountApiProtocol::Responses),
        backend_provider: None,
        models: vec![],
        enabled: true,
    };
    store
        .put_provider_account(
            &spec,
            0,
            Some(&protect_for_current_user(format!("upstream-key-{id}").as_bytes()).unwrap()),
        )
        .unwrap()
}

fn configure_pool(store: &Store) {
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            provider: "codex".into(),
            mode: AccountRoutingMode::ApiPool,
            account_id: None,
            revision: 0,
        })
        .unwrap();
}

#[tokio::test]
async fn anthropic_proxy_supports_native_beta_paths_and_uses_only_the_selected_key() {
    let fixture = upstream(StatusCode::OK, false).await;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open_path(temp.path().join("test.db")).unwrap();
    let mut spec = add_key(&store, temp.path(), 1, &fixture.url).spec;
    spec.id = Uuid::from_u128(2);
    spec.provider = "claude-code".into();
    spec.credential_environment = Some("ANTHROPIC_API_KEY".into());
    spec.api_protocol = Some(AccountApiProtocol::Anthropic);
    spec.base_url = Some(fixture.url.trim_end_matches("/v1").to_owned());
    store
        .put_provider_account(
            &spec,
            0,
            Some(&protect_for_current_user(b"anthropic-upstream-key").unwrap()),
        )
        .unwrap();
    store
        .set_provider_account_policy(&ProviderAccountPolicy {
            provider: "claude-code".into(),
            mode: AccountRoutingMode::ApiPool,
            account_id: None,
            revision: 0,
        })
        .unwrap();
    let session = Uuid::new_v4();
    let account = store
        .bind_provider_account(session, "claude-code", None, None)
        .unwrap()
        .unwrap();
    let proxy = AccountProxy::start(store, session, &account, false)
        .await
        .unwrap();
    let mut environment = lazoo_runtime::LaunchEnvironment::default();
    super::super::accounts::apply_endpoint(
        &mut environment,
        &spec,
        &proxy.endpoint(AccountApiProtocol::Anthropic),
    )
    .unwrap();
    let native_base = environment
        .values()
        .get(std::ffi::OsStr::new("ANTHROPIC_BASE_URL"))
        .unwrap()
        .to_string_lossy();
    let response = reqwest::Client::new()
        .post(format!("{native_base}/v1/messages?beta=true"))
        .header("x-api-key", proxy.token())
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({"model":"fixture","messages":[]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let requests = fixture.requests.lock().await;
    assert_eq!(requests[0].0, "anthropic-upstream-key");
    assert_eq!(requests[0].2, "/v1/messages?beta=true");
}

#[tokio::test]
async fn proxy_auth_failover_and_continuation_preserve_native_profile_ownership() {
    let rejected = upstream(StatusCode::TOO_MANY_REQUESTS, false).await;
    let accepted = upstream(StatusCode::OK, false).await;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open_path(temp.path().join("test.db")).unwrap();
    add_key(&store, temp.path(), 1, &rejected.url);
    add_key(&store, temp.path(), 2, &accepted.url);
    configure_pool(&store);
    let session = Uuid::new_v4();
    let profile = store
        .bind_provider_account(session, "codex", None, None)
        .unwrap()
        .unwrap();
    store
        .bind_provider_native_identity(session, "native-history")
        .unwrap();
    let proxy = AccountProxy::start(store.clone(), session, &profile, false)
        .await
        .unwrap();
    let url = format!(
        "{}/responses",
        proxy.endpoint(AccountApiProtocol::Responses)
    );
    let client = reqwest::Client::new();
    let unauthorized = client
        .post(&url)
        .json(&serde_json::json!({"model":"fixture"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert!(rejected.requests.lock().await.is_empty());
    let response = client
        .post(&url)
        .bearer_auth(proxy.token())
        .json(&serde_json::json!({"model":"fixture", "input":"hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), "{\"ok\":true}");
    assert_eq!(rejected.requests.lock().await.len(), 1);
    assert_eq!(accepted.requests.lock().await[0].0, "Bearer upstream-key-2");
    let binding = store.provider_account_binding(session).unwrap().unwrap();
    assert_eq!(binding.account_id, Some(Uuid::from_u128(1)));
    assert_eq!(binding.upstream_account_id, Some(Uuid::from_u128(2)));
    // The quota-limited key does not make its native transcript directory disappear.
    let resumed_session = Uuid::new_v4();
    let resumed = store
        .bind_provider_account(resumed_session, "codex", None, Some("native-history"))
        .unwrap()
        .unwrap();
    assert_eq!(resumed.spec.id, Uuid::from_u128(1));
    let resumed_proxy = AccountProxy::start(store.clone(), resumed_session, &resumed, true)
        .await
        .unwrap();
    let response = client
        .post(format!(
            "{}/responses",
            resumed_proxy.endpoint(AccountApiProtocol::Responses)
        ))
        .bearer_auth(resumed_proxy.token())
        .json(&serde_json::json!({"model":"fixture","previous_response_id":"owner-bound"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(accepted.requests.lock().await.len(), 2);
    assert_eq!(rejected.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn proxy_streams_first_chunk_before_completion_and_never_rotates_an_accepted_owner() {
    let first = upstream(StatusCode::OK, true).await;
    let other = upstream(StatusCode::OK, false).await;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open_path(temp.path().join("test.db")).unwrap();
    add_key(&store, temp.path(), 1, &first.url);
    add_key(&store, temp.path(), 2, &other.url);
    configure_pool(&store);
    let session = Uuid::new_v4();
    let account = store
        .bind_provider_account(session, "codex", None, None)
        .unwrap()
        .unwrap();
    let proxy = AccountProxy::start(store.clone(), session, &account, false)
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let url = format!(
        "{}/responses",
        proxy.endpoint(AccountApiProtocol::Responses)
    );
    let mut response = client
        .post(&url)
        .bearer_auth(proxy.token())
        .json(&serde_json::json!({"stream":true}))
        .send()
        .await
        .unwrap();
    let chunk = tokio::time::timeout(Duration::from_millis(150), response.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(chunk, Bytes::from_static(b"data: first\n\n"));
    drop(response); // Client cancellation must not trigger failover or replay.
    store
        .observe_provider_account(account.spec.id, 429, 1, 60)
        .unwrap();
    let response = client
        .post(&url)
        .bearer_auth(proxy.token())
        .json(&serde_json::json!({"previous_response_id":"continuation"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(first.requests.lock().await.len(), 1);
    assert!(other.requests.lock().await.is_empty());
    drop(proxy);
    tokio::task::yield_now().await;
    assert!(
        client
            .post(&url)
            .timeout(Duration::from_secs(1))
            .send()
            .await
            .is_err()
    );
}

#[tokio::test]
async fn transport_failure_and_explicit_continuation_are_never_replayed() {
    let refused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable = format!("http://{}/v1", refused.local_addr().unwrap());
    drop(refused);
    let fallback = upstream(StatusCode::OK, false).await;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open_path(temp.path().join("test.db")).unwrap();
    add_key(&store, temp.path(), 1, &unavailable);
    add_key(&store, temp.path(), 2, &fallback.url);
    configure_pool(&store);
    let session = Uuid::new_v4();
    let account = store
        .bind_provider_account(session, "codex", None, None)
        .unwrap()
        .unwrap();
    let proxy = AccountProxy::start(store.clone(), session, &account, false)
        .await
        .unwrap();
    let response = reqwest::Client::new()
        .post(format!(
            "{}/responses",
            proxy.endpoint(AccountApiProtocol::Responses)
        ))
        .bearer_auth(proxy.token())
        .json(&serde_json::json!({"input":"one delivery"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(fallback.requests.lock().await.is_empty());
    assert_eq!(
        store
            .provider_account_binding(session)
            .unwrap()
            .unwrap()
            .upstream_account_id,
        Some(account.spec.id)
    );
    assert!(contains_account_state(
        &serde_json::json!({"input":[{"file_id":"bound-file"}]})
    ));
    assert!(contains_account_state(
        &serde_json::json!({"previous_response_id":"bound-response"})
    ));
}

#[tokio::test]
#[ignore = "local performance evidence; no paid upstream requests"]
async fn measure_loopback_routing_overhead() {
    let fixture = upstream(StatusCode::OK, false).await;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open_path(temp.path().join("test.db")).unwrap();
    add_key(&store, temp.path(), 1, &fixture.url);
    configure_pool(&store);
    let session = Uuid::new_v4();
    let account = store
        .bind_provider_account(session, "codex", None, None)
        .unwrap()
        .unwrap();
    let proxy = AccountProxy::start(store, session, &account, false)
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let mut direct = Vec::new();
    let mut routed = Vec::new();
    for index in 0..35 {
        for (url, token, samples) in [
            (
                format!("{}/responses", fixture.url),
                "upstream-key-1",
                &mut direct,
            ),
            (
                format!(
                    "{}/responses",
                    proxy.endpoint(AccountApiProtocol::Responses)
                ),
                proxy.token(),
                &mut routed,
            ),
        ] {
            let start = Instant::now();
            let response = client
                .post(url)
                .bearer_auth(token)
                .json(&serde_json::json!({"model":"fixture","input":"hello"}))
                .send()
                .await
                .unwrap();
            assert!(response.status().is_success());
            response.bytes().await.unwrap();
            if index >= 5 {
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    direct.sort_by(f64::total_cmp);
    routed.sort_by(f64::total_cmp);
    println!(
        "LOCAL_PROXY_BENCHMARK samples=30 direct_median_ms={:.3} direct_p95_ms={:.3} routed_median_ms={:.3} routed_p95_ms={:.3} median_added_ms={:.3}",
        direct[15],
        direct[28],
        routed[15],
        routed[28],
        routed[15] - direct[15]
    );
}
