use super::*;

/// `snapshot()`, `any_down()` and `pending()` each took the
/// registry lock for themselves, so a supervisor flipping a verdict mid-document
/// could publish `"listeners": {"aux": "Serving"}` beside `"listeners_down": true` —
/// the red-over-green contradiction the badge work exists to prevent, re-opened
/// through read timing. The three fields are now three reads of ONE clone, so the
/// pair is checked against the map in the SAME document (never against the registry,
/// which would re-read the instant the pin is meant to audit) while a writer churns
/// the verdicts underneath. Falsifier: neuter `ListenerState::is_down` to
/// `matches!(self, Self::Serving)` and the agreement assertion fails on the map.
#[tokio::test]
async fn the_badge_booleans_are_always_the_published_maps_own_verdicts() {
    fn derived_from_the_published_map(doc: &Value) -> (bool, bool) {
        // The registry's serde shape, read exactly as the page reads it:
        // `"Serving"` / `"Pending"` as strings, `{"Down":{"reason":…}}` as the
        // externally-tagged struct variant.
        let listeners = doc["listeners"].as_object().expect("listeners map");
        (
            listeners.values().any(|state| state.get("Down").is_some()),
            listeners
                .values()
                .any(|state| state.as_str() == Some("Pending")),
        )
    }

    let api = live_state(BootTimings::default());
    let doc = status_document(&api).await.expect("status document");
    let (down, pending) = derived_from_the_published_map(&doc);
    assert!(
        pending && !down,
        "a freshly seeded registry is all-amber, never all-clear: {doc}"
    );
    assert_eq!(doc["listeners_down"], down, "unmixed boot: {doc}");

    api.listeners
        .mark_down(CategoryId::Aux, "port taken".to_string());
    api.listeners.mark_serving(CategoryId::Work);
    let doc = status_document(&api).await.expect("status document");
    let (down, pending) = derived_from_the_published_map(&doc);
    assert!(
        down && pending,
        "one Down and one still-unspoken category is red AND amber: {doc}"
    );
    assert_eq!(doc["listeners_down"], down, "mixed shape: {doc}");
    assert_eq!(doc["listeners_pending"], pending, "mixed shape: {doc}");

    // The timing leg: no document may ever contradict its own map, however the
    // verdicts move while it is being assembled.
    let churn_registry = api.listeners.clone();
    let churn = tokio::spawn(async move {
        for _ in 0..400 {
            churn_registry.mark_down(CategoryId::Aux, "flapping".to_string());
            tokio::task::yield_now().await;
            churn_registry.mark_serving(CategoryId::Aux);
            tokio::task::yield_now().await;
        }
    });
    for round in 0..120 {
        let doc = status_document(&api).await.expect("status document");
        let (down, pending) = derived_from_the_published_map(&doc);
        assert_eq!(
            doc["listeners_down"], down,
            "round {round} published a badge its own map denies: {doc}"
        );
        assert_eq!(
            doc["listeners_pending"], pending,
            "round {round} published a badge its own map denies: {doc}"
        );
    }
    churn.await.expect("the churner never panics");
}

/// `Reply::Done => (0, None)` handed the proxy
/// `127.0.0.1:0`, a port that belongs to no listener, and the failure surfaced as a
/// connect error three layers from its cause. The arm now refuses, and this
/// inspection tripwire keeps it that way: the fabricated zero may not come back into
/// `swap_for_port`, and the arm must still name an error class. Falsifier: restore
/// `(0, None)` and the body assertion fails.
#[test]
fn a_swap_that_names_no_port_is_refused_rather_than_forwarded_to_zero() {
    let source = include_str!("../proxy.rs");
    let start = source
        .find("async fn swap_for_port(")
        .expect("swap_for_port is still in this file");
    let body = &source[start..source[start..].find("\n}\n").expect("the function closes") + start];
    assert!(
        !body.contains("(0, None)"),
        "`swap_for_port` may not answer a swap with a fabricated port 0: {body}"
    );
    assert!(
        body.contains("Reply::Done") && body.contains("ApiError::Internal"),
        "the no-port reply must still be refused as an invariant violation: {body}"
    );
}

/// "Reads never wake" as a table:
/// `wakes_slot` answers `true` for exactly the three inference POSTs and
/// `false` for every other method/path pair the listeners and a curious
/// client can produce — reads, admin routes, and non-POST variants of the
/// inference paths. Falsifier: widen `wakes_slot` to wake on a GET or on a
/// fourth path and the matching row fails.
#[test]
fn only_the_three_inference_posts_wake_the_slot() {
    let table: Vec<(&str, &str, bool)> = vec![
        ("POST", "/v1/chat/completions", true),
        ("POST", "/v1/completions", true),
        ("POST", "/v1/embeddings", true),
        ("GET", "/v1/chat/completions", false),
        ("GET", "/v1/completions", false),
        ("GET", "/v1/embeddings", false),
        ("HEAD", "/v1/chat/completions", false),
        ("HEAD", "/v1/completions", false),
        ("HEAD", "/v1/embeddings", false),
        ("OPTIONS", "/v1/chat/completions", false),
        ("OPTIONS", "/v1/completions", false),
        ("OPTIONS", "/v1/embeddings", false),
        ("PUT", "/v1/embeddings", false),
        ("DELETE", "/v1/completions", false),
        ("GET", "/v1/models", false),
        ("POST", "/v1/models", false),
        ("GET", "/health", false),
        ("POST", "/health", false),
        ("GET", "/har/status", false),
        ("GET", "/har/jobs", false),
        ("POST", "/har/load", false),
        ("POST", "/har/unload", false),
        ("POST", "/har/rescan", false),
        ("POST", "/har/config/model", false),
        // A fourth inference-style path that is not wired must never wake.
        ("POST", "/v1/rerank", false),
        // A trailing slash is a different path; the router 404s it, the
        // predicate must not wake on it either.
        ("POST", "/v1/chat/completions/", false),
        ("POST", "", false),
        ("GET", "", false),
    ];
    for (method, path, wakes) in table {
        let method = Method::from_bytes(method.as_bytes()).expect("valid method literal");
        assert_eq!(wakes_slot(&method, path), wakes, "{method} {path}");
    }
}

/// Route-table falsifier: the predicate's literal list and the category
/// listener route table must agree on the exact wake set, in both
/// directions — wiring any other route to `proxy` fails the per-route leg,
/// and adding any other waking path to `wakes_slot` fails the literal-list
/// and proxy-wiring legs. Falsifier: add a fourth path to `wakes_slot`
/// without touching the route table and the literal-list leg fails; re-wire
/// `/v1/models` to `proxy` and the per-route leg fails.
#[test]
fn the_wake_set_is_exactly_the_proxy_routes() {
    fn quoted_literals(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find('"') {
            rest = &rest[open + 1..];
            let Some(close) = rest.find('"') else { break };
            out.push(&rest[..close]);
            rest = &rest[close + 1..];
        }
        out
    }
    let source = include_str!("../proxy.rs");
    let start = source
        .find("fn wakes_slot(")
        .expect("wakes_slot is still in this file");
    let predicate =
        &source[start..start + source[start..].find("\n}\n").expect("wakes_slot closes")];
    let mut woken = quoted_literals(predicate);
    woken.sort_unstable();
    assert_eq!(
        woken,
        vec!["/v1/chat/completions", "/v1/completions", "/v1/embeddings"],
        "`wakes_slot` must list exactly the three inference paths and nothing else"
    );

    let router_at = source
        .find("pub fn category_router(")
        .expect("category_router is still in this file");
    let rest = &source[router_at..];
    let table = &rest[..rest
        .find(".layer((")
        .expect("the route entries end at the first layer")];
    // Absence leg (B4R-01): the enumeration below counts only `.route(`
    // entries, so a structural combinator inside the scanned region would
    // wake a slot without ever reaching the per-route checks or `total`.
    // A future `.nest(`/`.merge(`/`.fallback(`/`.route_method(` wire-up
    // must fail here, not escape the pin.
    assert!(
        !table.contains(".nest(")
            && !table.contains(".merge(")
            && !table.contains(".fallback(")
            && !table.contains(".route_method("),
        "route shape must stay enumerable by `.route(`"
    );
    let mut proxied: Vec<&str> = Vec::new();
    let mut total = 0;
    for entry in table.split(".route(").skip(1) {
        total += 1;
        let open = entry.find('"').expect("route path is quoted") + 1;
        let path = &entry[open..open + entry[open..].find('"').expect("path quote closes")];
        let (method, method_token) = match (entry.find("get("), entry.find("post(")) {
            (Some(g), Some(p)) if p < g => (Method::POST, "post("),
            (Some(_), Some(_)) => (Method::GET, "get("),
            (Some(_), None) => (Method::GET, "get("),
            (None, Some(_)) => (Method::POST, "post("),
            (None, None) => panic!("route {path} wires no known method combinator"),
        };
        let handler_at = entry.find(method_token).expect("method token") + method_token.len();
        let handler =
            &entry[handler_at..handler_at + entry[handler_at..].find(')').expect("handler closes")];
        assert_eq!(
            wakes_slot(&method, path),
            handler == "proxy",
            "route {method} {path} ({handler}) must agree with `wakes_slot`"
        );
        if handler == "proxy" {
            proxied.push(path);
        }
    }
    assert_eq!(
        total, 5,
        "the category-listener route table changed shape; re-read the wake pin: {table}"
    );
    proxied.sort_unstable();
    assert_eq!(
        proxied,
        vec!["/v1/chat/completions", "/v1/completions", "/v1/embeddings"],
        "`proxy` must be wired to exactly the three waking routes"
    );
}

/// The memory-bar sum (server side, where the honesty rule lives): it sums
/// the per-slot `engine_report` figures the scheduler anchored — one figure per
/// model even when the same container occupies two slots — and answers `None`
/// (never `0.0`) when nobody reported, so a silent container cannot render as
/// "reserved 0 GiB" next to a busy card. The engine/attribution gates themselves
/// are pinned at their source (`only_an_engine_report_anchor_reaches_the_snapshot_slot`,
/// scheduler); this pins the fold-over-slots above them.
#[test]
fn the_container_reported_sum_dedupes_models_and_never_fakes_zero() {
    let now = Instant::now();
    let reported = |model: &str, gib: Option<f64>| SlotStatus {
        container_reported_gb: gib,
        ..occupied_slot(model, 9_100, now)
    };
    let empty = SlotStatus {
        model: None,
        state: "empty".to_string(),
        ..occupied_slot("empty", 9_101, now)
    };
    let document = |slots: Vec<SlotStatus>| {
        container_reported_gb(&SchedulerStatus {
            slots,
            jobs: Vec::new(),
            catalog_errors: Vec::new(),
            stranded_containers: None,
            fit_validated: Default::default(),
        })
    };
    assert_eq!(document(vec![empty, reported("quiet", None)]), None);
    assert_eq!(
        document(vec![reported("a", Some(20.0)), reported("b", Some(11.25))]),
        Some(31.25)
    );
    assert_eq!(
        document(vec![
            reported("twin", Some(30.0)),
            reported("twin", Some(30.0))
        ]),
        Some(30.0),
        "one container on two slots is one reservation"
    );
}

#[test]
fn no_fabricated_instants_survive_short_uptime() {
    // The removed fallback sorted untouched occupants by a
    // fabricated `Instant::now() - 3600s`, which panics outright on a host
    // younger than an hour. Fixture instants are FORWARD offsets of `now`
    // (`Instant + Duration` cannot underflow) so the test never panics on a
    // young host — the same panic the finding condemned. The pin is the
    // REAL picker's winner: pre-fix both slots collapse to the fabricated
    // hour and the last-evaluated slot (`older`, snapshot order reversed
    // against load order) wins the jitter tie; post-fix the actual
    // `loaded_at` ordering makes `newer` win, on any uptime.
    let api = unit_state();
    let now = Instant::now();
    // 20-minute separation is far above QueryPerformanceCounter jitter.
    let t_older = now + Duration::from_secs(600);
    let t_newer = now + Duration::from_secs(1_200);
    let status = SchedulerStatus {
        slots: vec![
            occupied_slot("newer", 9002, t_newer),
            occupied_slot("older", 9001, t_older),
        ],
        jobs: Vec::new(),
        catalog_errors: Vec::new(),
        stranded_containers: None,
        fit_validated: Default::default(),
    };
    let picked = pick_loaded_occupant(&api, &status, CategoryId::Aux)
        .expect("an occupied category must resolve");
    assert_eq!(
        picked.0, "newer",
        "untouched occupants sort by ACTUAL load time"
    );
    assert_eq!(picked.1, 9002);
    assert_eq!(picked.2.as_deref(), Some("mock-model"));
}

#[test]
fn mru_touch_wins_over_a_fresher_load() {
    let api = unit_state();
    // Load stamps sit NEAR now (not in the future: an in-process future stamp
    // would outrank a touch landing at ~now, inverting reality — real loads
    // always precede their touches). `hot` loaded last, yet the touch on
    // `cold` must outrank it. Backward offsets stay under a minute so the
    // fixtures are safe on any host uptime; the touch lands after them.
    let status = SchedulerStatus {
        slots: vec![
            occupied_slot("cold", 9001, Instant::now()),
            occupied_slot(
                "hot",
                9002,
                Instant::now()
                    .checked_sub(Duration::from_millis(150))
                    .expect("invariant: process start precedes first fixture stamp"),
            ),
        ],
        jobs: Vec::new(),
        catalog_errors: Vec::new(),
        stranded_containers: None,
        fit_validated: Default::default(),
    };
    api.touch("cold");
    let picked = pick_loaded_occupant(&api, &status, CategoryId::Aux)
        .expect("an occupied category must resolve");
    assert_eq!(picked.0, "cold", "a proxied model outranks a fresher load");
}

#[test]
fn other_category_and_dead_ports_resolve_to_nothing() {
    let api = unit_state();
    let now = Instant::now();
    let foreign = SlotStatus {
        category: CategoryId::Work,
        ..occupied_slot("elsewhere", 9003, now)
    };
    let portless = SlotStatus {
        port: None,
        ..occupied_slot("unbound", 9004, now)
    };
    let zero = SlotStatus {
        port: Some(0),
        ..occupied_slot("zombie", 0, now)
    };
    let empty = SlotStatus {
        category: CategoryId::Aux,
        index: 1,
        model: None,
        port: None,
        pid: None,
        need_gib: None,
        idle_s: None,
        serve_id: None,
        loading_job: None,
        state: "empty".to_string(),
        launched_at: None,
        stale_config: None,
        container_reported_gb: None,
        booked_ram_gb: None,
        ram_cost_basis: None,
        ram_source: None,
        loaded_at: now,
    };
    let status = SchedulerStatus {
        slots: vec![foreign, portless, zero, empty],
        jobs: Vec::new(),
        catalog_errors: Vec::new(),
        stranded_containers: None,
        fit_validated: Default::default(),
    };
    assert!(
        pick_loaded_occupant(&api, &status, CategoryId::Aux).is_none(),
        "wrong-category, portless, and port-0 slots must not resolve"
    );
}

#[tokio::test]
async fn connect_failure_is_the_only_restart_class() {
    // Evidence for the connect class: the mock death is
    // a `handle.abort()` that drops the listener socket, so the port REFUSES
    // and `send()` yields `is_connect()` — exactly what this classifier maps
    // to the single sanctioned reap + reload + retry.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind free port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    let error = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .json(&json!({"messages": []}))
        .send()
        .await
        .expect_err("nothing accepts on the released port");
    assert!(
        error.is_connect(),
        "dead port must be connect-class: {error}"
    );
    let action = classify_send_failure(&error, port).expect("connect-class retries once");
    assert_eq!(action, RetryAction::Restart);
}

/// A total `ClientBuilder::timeout` covers the response body, so
/// it cut a long SSE generation mid-stream after the 200. The production client
/// must carry a per-read deadline, not a total one. Falsifier: put `.timeout(...)`
/// back into the builder and this tripwire fails.
#[test]
fn the_proxy_client_has_no_total_deadline() {
    let source = include_str!("../mod.rs");
    let start = source
        .find("proxy: reqwest::Client::builder()")
        .expect("the proxy client builder is still in this file");
    let end = source[start..]
        .find(".build()")
        .expect("the builder still finishes with build()")
        + start;
    let builder = &source[start..end];
    assert!(
        builder.contains(".read_timeout("),
        "the proxy needs a per-read deadline: {builder}"
    );
    assert!(
        !builder.contains(".timeout("),
        "a total timeout covers the SSE body and truncates live generations: {builder}"
    );
}

#[tokio::test]
async fn timeout_of_a_live_engine_never_restarts() {
    // A server that accepts but never answers produces a real reqwest
    // timeout — the class that used to mean "engine died" and reaped a
    // healthy engine mid-generation. It must now be a typed 502 instead.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind free port");
    let port = listener.local_addr().expect("local addr").port();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()
        .expect("plain-HTTP client with a timeout");
    let error = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .json(&json!({"messages": []}))
        .send()
        .await
        .expect_err("the socket is accepted but never answered");
    assert!(
        error.is_timeout(),
        "hanging server must be timeout-class: {error}"
    );
    let err = classify_send_failure(&error, port).expect_err("timeouts never restart");
    assert_eq!(err.status(), StatusCode::BAD_GATEWAY);
    let reason = match &err {
        ApiError::Upstream { reason } => reason.clone(),
        other => panic!("expected Upstream, got {other:?}"),
    };
    assert!(
        reason.contains(&format!("engine on :{port} timed out"))
            && reason.contains("not restarting a live engine"),
        "reason must name the class and the refusal to reap: {reason}"
    );
    drop(listener);
}

#[test]
fn timeout_precedes_connect_when_both_flags_are_true() {
    // Falsifier: the both-true row is RED under the old
    // connect-first order. The table pins the decision on the flags alone.
    assert_eq!(
        classify_send_failure_flags(false, false),
        SendFailureClass::Other,
        "neither flag must stay a plain forward failure"
    );
    assert_eq!(
        classify_send_failure_flags(true, false),
        SendFailureClass::Timeout,
        "timeout-only must be the 502 class"
    );
    assert_eq!(
        classify_send_failure_flags(false, true),
        SendFailureClass::Connect,
        "connect-only (refused/reset) must stay the restart class"
    );
    assert_eq!(
        classify_send_failure_flags(true, true),
        SendFailureClass::Timeout,
        "both flags must be the timeout class, never a reap"
    );
}

#[tokio::test]
async fn non_connect_errors_are_upstream_never_death() {
    // A redirect loop is the finding's named non-death trigger ("a redirect
    // loop or a request-body error"): the engine was alive and answering, so
    // the class must never be read as death and must never restart. Raw
    // sockets speak just enough HTTP/1.1 to bounce the client forever.
    let looping = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("redirect loop server binds");
    let port = looping.local_addr().expect("loop addr").port();
    let server = tokio::spawn(async move {
        let reply = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/loop\r\n\
                 Content-Length: 0\r\nConnection: keep-alive\r\n\r\n"
        );
        while let Ok((mut stream, _)) = looping.accept().await {
            let reply = reply.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    if stream.write_all(reply.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let error = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(4))
        .build()
        .expect("plain-HTTP client")
        .post(format!("http://127.0.0.1:{port}/loop"))
        .json(&json!({"messages": []}))
        .send()
        .await
        .expect_err("the redirect loop must exhaust the policy");
    assert!(
        !error.is_connect(),
        "a live answering server is not death: {error}"
    );
    let err = classify_send_failure(&error, port).expect_err("non-connect never restarts");
    match err {
        ApiError::Upstream { reason } => assert!(
            reason.starts_with("forward failed:"),
            "reason must name the class: {reason}"
        ),
        other => panic!("expected Upstream, got {other:?}"),
    }
    server.abort();
}
