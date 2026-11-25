use super::*;

/// The four guard refusals used to answer and vanish while
/// benign 200s filled the same ring the operator reads after a bad boot. A refusal
/// must leave a line, but a flood must not turn that line into an eviction engine —
/// these are precisely the responses an attacker produces in bulk. One line per
/// `(class, path)`, the repeats counted, and the count published by the next
/// successful command. Falsifier: report unconditionally (drop the dedup branch in
/// `note_guard_refusal`) and the first count assertion fails with 24 lines; never
/// re-arm and the last assertion fails with 0.
#[tokio::test]
async fn guard_refusals_leave_one_line_per_class_and_path_not_one_per_packet() {
    crate::observability::set_extended_logging(false);
    // Live actor: the recovery leg needs a poll that actually answers 2xx, and a
    // 5xx would let the window stay shut while looking like it re-armed.
    let api = Arc::new(live_state(BootTimings::default()));
    // The install helper warms the reporting callsites — *proven* to deliver —
    // before this thread subscribes (see [`guard_capture_session`]): their
    // process-wide `Interest` used to be established by an un-asserted warm-up
    // emission that a co-scheduled, capture-less sibling guard pin could win,
    // which is the D9 flake — the flood's one line absent while its tower
    // lines were present.
    let session = guard_capture_session().await;

    async fn probe(
        api: Arc<ApiState>,
        path: &'static str,
        origin: Option<&'static str>,
        json: bool,
    ) -> StatusCode {
        let mut builder = Request::post(path).header("host", "127.0.0.1:8708");
        if let Some(origin) = origin {
            builder = builder.header("origin", origin);
        }
        if json {
            builder = builder.header("content-type", "application/json");
        }
        control_router(api)
            .oneshot(builder.body(Body::empty()).expect("probe request"))
            .await
            .expect("in-process probe")
            .status()
    }

    for _ in 0..20 {
        assert_eq!(
            probe(
                Arc::clone(&api),
                "/har/rescan",
                Some("http://evil.example"),
                true
            )
            .await,
            StatusCode::FORBIDDEN,
            "the cross-site POST is refused before anything else can see it"
        );
    }
    assert_eq!(
        probe(Arc::clone(&api), "/har/rescan", None, false).await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "same path, different class: the dedup unit is (class, path), not path"
    );
    assert_eq!(
        probe(
            Arc::clone(&api),
            "/har/unload",
            Some("http://evil.example"),
            true
        )
        .await,
        StatusCode::FORBIDDEN,
        "same class, different path: a second path is its own line"
    );
    // The fourth class, from a state that demands the token: 401 is what a guess
    // produces, so it must be visible — and the secret must not be.
    const OFFERED: &str = "sekrit-never-in-the-ring";
    let guarded = Arc::new(ApiState::new(
        AppConfig {
            control_token: Some(OFFERED.to_string()),
            ..AppConfig::default()
        },
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    ));
    for _ in 0..2 {
        let response = control_router(Arc::clone(&guarded))
            .oneshot(
                Request::post("/har/rescan")
                    .header("host", "127.0.0.1:8708")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer nope")
                    .body(Body::empty())
                    .expect("probe request"),
            )
            .await
            .expect("in-process probe");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let lines = session.stop();
    assert_eq!(
        refused_lines(&lines),
        4,
        "20 identical refusals are one line, the two distinct (class, path) pairs \
             are one line each, and the token guesses are one more: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("guard refused")
            && line.contains("refusal=\"token\"")
            && line.contains("status=401")),
        "the token class renders its own line: {lines:?}"
    );
    assert!(
        lines.iter().all(|line| !line.contains(OFFERED)),
        "the ring is readable by the page: the control token may not be in it: \
             {lines:?}"
    );

    // A routine poll answers 200 but is not a command: the page's 2 s timer must
    // never re-arm a flood into a line every tick.
    let poll = control_router(Arc::clone(&api))
        .oneshot(
            Request::get("/har/status")
                .header("host", "127.0.0.1:8708")
                .body(Body::empty())
                .expect("poll request"),
        )
        .await
        .expect("in-process poll");
    assert_eq!(poll.status(), StatusCode::OK);
    let quiet = guard_capture_session().await;
    assert_eq!(
        control_router(Arc::clone(&api))
            .oneshot(
                Request::get("/health")
                    .header("host", "127.0.0.1:8708")
                    .body(Body::empty())
                    .expect("health request"),
            )
            .await
            .expect("in-process health read")
            .status(),
        StatusCode::OK,
        "a successful command closes the window"
    );
    let closed = quiet.stop();
    let cleared = closed
        .iter()
        .filter(|line| line.contains("guard refusals cleared"))
        .collect::<Vec<_>>();
    assert_eq!(cleared.len(), 1, "exactly one summary line: {closed:?}");
    let summary = &cleared[0];
    for expected in [
        "origin on /har/rescan (+19 suppressed)",
        "content-type on /har/rescan",
        "origin on /har/unload",
    ] {
        assert!(
            summary.contains(expected),
            "the summary must name what the flood hid, with its count — missing \
                 {expected:?} in {summary}"
        );
    }

    let rearmed = guard_capture_session().await;
    for _ in 0..3 {
        assert_eq!(
            probe(
                Arc::clone(&api),
                "/har/rescan",
                Some("http://evil.example"),
                true
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        refused_lines(&rearmed.stop()),
        1,
        "the window re-armed after the command, so a fresh flood is still visible"
    );
}

fn refused_lines(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|line| line.contains("guard refused"))
        .count()
}

/// The three guard reporting callsites: `note_guard_refusal`'s `Report` and
/// `Fold` arms and `note_guard_recovery`'s summary. No guard pin calls this by
/// hand anymore: [`guard_capture_session`] warms through this before it
/// subscribes, so the structural install — pinned by
/// `the_guard_capture_install_warms_before_it_subscribes` — is the guarantee.
async fn warm_guard_reporting_callsites() {
    warm_capture_lines(
        &[
            "guard refused",
            "guard refusal window full",
            "guard refusals cleared",
        ],
        || async {
            let warm = warm_state(Some("warm-up-token"));
            // Past the per-class cap in one class: the cap's worth of `Report`
            // lines and the one `Fold` line, from a class no pin asserts about.
            for index in 0..=GUARD_TRACKED_PATHS_PER_CLASS {
                let response = control_router(Arc::clone(&warm))
                    .oneshot(
                        Request::post(format!("/warm/{index}"))
                            .header("host", "127.0.0.1:8708")
                            .header("origin", "http://warm.example")
                            .header("content-type", "application/json")
                            .body(Body::empty())
                            .expect("warm-up request"),
                    )
                    .await
                    .expect("in-process warm-up");
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
            }
            // A successful command after refusals: the recovery summary line.
            let health = control_router(Arc::clone(&warm))
                .oneshot(
                    Request::get("/health")
                        .header("host", "127.0.0.1:8708")
                        .header("x-har-token", "warm-up-token")
                        .body(Body::empty())
                        .expect("warm-up health read"),
                )
                .await
                .expect("in-process warm-up");
            assert_eq!(health.status(), StatusCode::OK);
        },
    )
    .await;
}

/// The one capture install for any pin that asserts on guard reporting lines:
/// it warms the three callsites through [`warm_guard_reporting_callsites`]
/// *first*, then subscribes this thread to the measured capture. The order is
/// the one every by-hand pair used (warm on a throwaway state on this thread,
/// `Session::start` after — a current-thread runtime keeps polling the future
/// here), and the warm-up is idempotent: each run uses a fresh throwaway
/// state, so it never spends the budget or fold tallies a pin measures.
/// Subscribing without warming is thereby not expressible for guard pins;
/// `the_guard_capture_install_warms_before_it_subscribes` pins the order and
/// `a_guard_capture_installs_warmed_with_no_by_hand_warm_up` shows the line
/// arriving in a capture that warmed nothing itself.
async fn guard_capture_session() -> crate::tray::testlog::Session {
    warm_guard_reporting_callsites().await;
    crate::tray::testlog::Session::start()
}

/// The token posture: reporting a refusal must never
/// mean publishing the secret, and the flood must still count as one report. The
/// assertions read the report bookkeeping and the wire bodies directly rather than
/// a captured subscriber, because which thread's thread-local default is active
/// under the parallel harness is not this pin's business (the rendering of the line
/// itself is asserted by `guard_refusals_leave_one_line_per_class_and_path_not_one_per_packet`,
/// which owns the single capture session). Falsifier: build the reported sentence
/// from the presented credential and the absence assertions fail.
#[tokio::test]
async fn a_refused_token_is_counted_once_and_never_carries_the_token() {
    crate::observability::set_extended_logging(false);
    const OFFERED: &str = "sekrit-token-value-never-logged";
    let config = AppConfig {
        control_token: Some(OFFERED.to_string()),
        ..AppConfig::default()
    };
    let api = Arc::new(ApiState::new(
        config,
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    ));
    let mut bodies: Vec<String> = Vec::new();
    for _ in 0..5 {
        let response = control_router(Arc::clone(&api))
            .oneshot(
                Request::post("/har/rescan")
                    .header("host", "127.0.0.1:8708")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .expect("probe request"),
            )
            .await
            .expect("in-process probe");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("refusal body");
        bodies.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    assert_eq!(
        api.guard_report(),
        vec![("token".to_string(), "/har/rescan".to_string(), 4)],
        "five guesses at the token are one report and a count of four"
    );
    for body in &bodies {
        assert!(
            !body.contains(OFFERED),
            "the 401 body may not echo what the ring would have carried: {body}"
        );
    }
    assert!(
        !format!("{:?}", api.guard_report()).contains(OFFERED),
        "the evidence trail names the class and the route, never the credential"
    );

    let health = control_router(Arc::clone(&api))
        .oneshot(
            Request::get("/health")
                .header("host", "127.0.0.1:8708")
                .header("x-har-token", OFFERED)
                .body(Body::empty())
                .expect("health request"),
        )
        .await
        .expect("in-process health read");
    assert_eq!(health.status(), StatusCode::OK);
    assert!(
        api.guard_report().is_empty(),
        "a successful command publishes and re-arms the window: {:?}",
        api.guard_report()
    );
}

/// The observable property of the structural install.
/// This pin names no warm-up yet still sees the guard line after the
/// capture-less first-touch hazard is re-created on purpose: one refusal is reported with no capture
/// installed on this thread (the capture-less first touch of the original
/// flake), and only then does [`guard_capture_session`] subscribe. What makes
/// it red: a tracing-core regression that revives the swallow — a first touch
/// with no subscriber caching an interest no later capture can undo. As
/// measured on the locked tracing dependencies (scratch binary plus this pin
/// under the dropped-warm falsifier) the
/// current version no longer swallows in any shape a test can reach, so
/// deleting the warm from `guard_capture_session` passes this pin; the order
/// itself is falsified by `the_guard_capture_install_warms_before_it_subscribes`,
/// which is red when the warm is dropped and red when it is reordered.
#[tokio::test]
async fn a_guard_capture_installs_warmed_with_no_by_hand_warm_up() {
    crate::observability::set_extended_logging(false);
    async fn refuse(api: &Arc<ApiState>) -> StatusCode {
        control_router(Arc::clone(api))
            .oneshot(
                Request::post("/har/rescan")
                    .header("host", "127.0.0.1:8708")
                    .header("origin", "http://evil.example")
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .expect("probe request"),
            )
            .await
            .expect("in-process probe")
            .status()
    }
    // The hazard first: a refusal reported while this thread has no capture —
    // exactly the capture-less first touch the D9 flake was. Its own throwaway
    // state, so it spends nothing from the measured window's budget.
    let unwatched = warm_state(None);
    assert_eq!(refuse(&unwatched).await, StatusCode::FORBIDDEN);
    // No warm-up call between these lines: the install carries it. A fresh
    // state so the refusal is the window's first Report for this (class, path).
    let api = Arc::new(unit_state());
    let session = guard_capture_session().await;
    assert_eq!(refuse(&api).await, StatusCode::FORBIDDEN);
    let lines = session.stop();
    assert!(
        lines.iter().any(|line| line.contains("guard refused")),
        "the guard line must reach a capture whose test never warmed by hand: \
             {lines:?}"
    );
}

/// The behavioral pin above can only watch the order
/// it is run in, so the order itself is pinned at the source (the repo's
/// established `include_str!` seam): inside `guard_capture_session`'s body the
/// warm-up token must appear before the subscribe token. Comments are stripped
/// before the match, so prose cannot satisfy the order. Falsifier: move
/// `Session::start()` above the warm-up and the order assert fires; delete the
/// warm-up call and the expect fires. A guard pin that bypasses the helper is
/// caught here the same way once it warms inside its own install, or leaves
/// the structural guarantee vacuous for its lines.
#[test]
fn the_guard_capture_install_warms_before_it_subscribes() {
    let source = include_str!("guard.rs");
    let start = source
        .find("async fn guard_capture_session(")
        .expect("the guard-capture install helper must stay");
    let rest = &source[start..];
    let end = rest
        .find("\n}\n")
        .expect("the install helper closes at column 0");
    let body = rest[..end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let warm = body
        .find("warm_guard_reporting_callsites()")
        .expect("the install helper must warm the reporting callsites");
    let subscribe = body
        .find("Session::start()")
        .expect("the install helper must install the capture subscriber");
    assert!(
        warm < subscribe,
        "`guard_capture_session` may not subscribe before it warms (D9): {body}"
    );
}

/// Bounding *repeats* per `(class, path)` did not
/// bound the flood, because three of the four classes are refused **pre-auth on any
/// path** — `reject_reason` runs ahead of `auth_reason` and consults neither the
/// token nor route membership. So N bogus paths were N ring lines (ring turnover,
/// operator's post-mortem gone) and N map entries (unbounded state). The window is
/// now bounded on cardinality too: `GUARD_TRACKED_PATHS_PER_CLASS` named paths per
/// class, then one fold line, then silence, and the collapsed count rides the
/// recovery line.
///
/// Proves the stated bound, not a smaller one: 3× the threshold distinct paths from
/// an unauthenticated client cost the ring exactly `cap + 1` origin lines, the state
/// holds exactly `cap` keys, and the token class keeps its own budget (a flood on one
/// wall cannot mute the evidence for another).
///
/// Falsifier: revert `note_guard_refusal` to the unbounded map (drop the cap branch
/// and fold, report whenever the key is new) and the origin-count assertion fails with
/// 24 lines, the `guard_report().len()` assertion fails with 24 keys, and the fold
/// line disappears.
#[tokio::test]
async fn a_distinct_path_flood_costs_a_constant_number_of_lines() {
    crate::observability::set_extended_logging(false);
    const OFFERED: &str = "sekrit-never-in-the-ring";
    let api = Arc::new(ApiState::new(
        AppConfig {
            control_token: Some(OFFERED.to_string()),
            ..AppConfig::default()
        },
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    ));

    async fn probe(api: &Arc<ApiState>, path: &str, origin: Option<&str>) -> StatusCode {
        let mut builder = Request::post(path)
            .header("host", "127.0.0.1:8708")
            .header("content-type", "application/json");
        if let Some(origin) = origin {
            builder = builder.header("origin", origin);
        }
        control_router(Arc::clone(api))
            .oneshot(builder.body(Body::empty()).expect("probe request"))
            .await
            .expect("in-process probe")
            .status()
    }

    // Both reporting callsites this pin asserts on (`Report`, `Fold`) and the
    // recovery summary are proven to deliver into a capture before the measured
    // window opens — the install helper warms on a throwaway state, so none of
    // this spends the budget or the fold tally the assertions below read
    // (see [`guard_capture_session`]).
    let session = guard_capture_session().await;
    let flood = 3 * GUARD_TRACKED_PATHS_PER_CLASS;
    for index in 0..flood {
        assert_eq!(
            probe(
                &api,
                &format!("/flood/{index}"),
                Some("http://evil.example")
            )
            .await,
            StatusCode::FORBIDDEN,
            "a foreign Origin is refused before the token, on any path"
        );
    }
    // A different class keeps its own budget: a path flood on one wall may not mute
    // the evidence for token guessing on another.
    for _ in 0..2 {
        assert_eq!(
            probe(&api, "/har/rescan", None).await,
            StatusCode::UNAUTHORIZED
        );
    }
    let lines = session.stop();

    let origin_lines = lines
        .iter()
        .filter(|line| line.contains("guard refused") && line.contains("refusal=\"origin\""))
        .count();
    assert_eq!(
        origin_lines, GUARD_TRACKED_PATHS_PER_CLASS,
        "an attacker chose {flood} distinct paths; the ring may only pay for the \
             tracked cap: {lines:?}"
    );
    let fold_lines = lines
        .iter()
        .filter(|line| line.contains("guard refusal window full"))
        .count();
    assert_eq!(
        fold_lines, 1,
        "the overflow collapses to exactly one fold line for the class: {lines:?}"
    );
    let fold = lines
        .iter()
        .find(|line| line.contains("guard refusal window full"))
        .expect("the fold line exists");
    assert!(
        fold.contains("refusal=\"origin\"")
            && fold.contains(&format!("tracked={GUARD_TRACKED_PATHS_PER_CLASS}"))
            && fold.contains("folded=1"),
        "the fold line is spent on the first collapsed path and names the cap it \
             applied; the running total is the recovery line's job (asserted below): {fold}"
    );
    assert_eq!(
        refused_lines(&lines),
        GUARD_TRACKED_PATHS_PER_CLASS + 1,
        "cap + one token line, not one line per packet: {lines:?}"
    );
    assert!(
        refused_lines(&lines) <= 4 * (GUARD_TRACKED_PATHS_PER_CLASS + 1),
        "the documented worst-case window cost is 4 classes x (cap + fold): {:?}",
        refused_lines(&lines)
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("guard refused") && line.contains("status=401"))
            .count(),
        1,
        "the token leg is untouched by the Origin flood: {lines:?}"
    );
    // The untracked paths really are absent, not merely uncounted.
    let last = format!("/flood/{}", flood - 1);
    assert!(
        lines.iter().all(|line| !line.contains(&last)),
        "{last} was past the cap and must not appear: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("guard refused") && line.contains("path=/flood/0 ")),
        "the first {GUARD_TRACKED_PATHS_PER_CLASS} paths keep their detail: {lines:?}"
    );

    // Bounded state, not merely bounded lines: this is what makes the bound provable
    // rather than lucky, and no attacker-chosen path can push it further.
    let tracked_origin: Vec<_> = api
        .guard_report()
        .into_iter()
        .filter(|(class, _, _)| class == "origin")
        .collect();
    assert_eq!(
        tracked_origin.len(),
        GUARD_TRACKED_PATHS_PER_CLASS,
        "the map is capped per class: {:?}",
        api.guard_report()
    );
    assert!(
        api.guard_report().len() <= 4 * GUARD_TRACKED_PATHS_PER_CLASS,
        "state is bounded by classes x cap, whatever the flood: {:?}",
        api.guard_report()
    );
    assert_eq!(
        api.guard_folds()
            .into_iter()
            .filter(|(class, _)| class == "origin")
            .collect::<Vec<_>>(),
        vec![(
            "origin".to_string(),
            (flood - GUARD_TRACKED_PATHS_PER_CLASS) as u64
        )],
        "the collapsed paths are counted, once each (the warm-up runs against its \
             own state, so nothing here is shared with it): {:?}",
        api.guard_folds()
    );

    // Recovery semantics survive the cap: one line, and it says what was hidden.
    let recovery = guard_capture_session().await;
    let health = control_router(Arc::clone(&api))
        .oneshot(
            Request::get("/health")
                .header("host", "127.0.0.1:8708")
                .header("x-har-token", OFFERED)
                .body(Body::empty())
                .expect("health request"),
        )
        .await
        .expect("in-process health read");
    assert_eq!(health.status(), StatusCode::OK);
    let closed = recovery.stop();
    let cleared = closed
        .iter()
        .filter(|line| line.contains("guard refusals cleared"))
        .collect::<Vec<_>>();
    assert_eq!(cleared.len(), 1, "exactly one summary line: {closed:?}");
    assert!(
        cleared[0].contains("origin: +16 further paths folded"),
        "the fold is published where the operator will read it: {}",
        cleared[0]
    );
    assert!(api.guard_report().is_empty() && api.guard_folds().is_empty());

    // Re-armed: a fresh distinct path reports again, and an attacker-chosen path of
    // any length costs only the tracked budget (the key and the line agree).
    let long = format!("/long/{}", "x".repeat(4 * GUARD_TRACKED_PATH_CHARS));
    let rearmed = guard_capture_session().await;
    assert_eq!(
        probe(&api, &long, Some("http://evil.example")).await,
        StatusCode::FORBIDDEN
    );
    let rearmed_lines = rearmed.stop();
    assert_eq!(
        refused_lines(&rearmed_lines),
        1,
        "the window re-armed after the command: {rearmed_lines:?}"
    );
    let stored = api
        .guard_report()
        .first()
        .expect("the re-armed window holds the refusal")
        .1
        .clone();
    assert_eq!(
        stored.chars().count(),
        GUARD_TRACKED_PATH_CHARS,
        "an attacker-chosen path cannot become an allocation channel: {stored}"
    );
    let reported = rearmed_lines
        .iter()
        .find(|line| line.contains("guard refused"))
        .expect("the re-armed window prints its line");
    assert!(
        stored.ends_with("...") && reported.contains(&stored),
        "the line names the same elided path the map keys on: {reported}"
    );
}

/// The guard window re-armed on ANY unauthenticated success, and every
/// `PUBLIC_PATHS` member answers 200 without a token in both postures — so an attacker
/// alternating refused POSTs with free `GET /` packets got one `guard refused`
/// line plus one `guard refusals cleared` line per two packets, turning the ring
/// over at request rate: the exact eviction flood the cardinality cap
/// was built to end. A page or asset is not a
/// command. Falsifier: drop the `PUBLIC_PATHS` leg of the guard's re-arm
/// condition and the origin-line count fails with one line per packet, the
/// fold tally never accumulates, and the no-cleared-lines leg fails with a
/// summary per interleave.
#[tokio::test]
async fn a_public_page_success_never_rearms_the_refusal_window() {
    crate::observability::set_extended_logging(false);
    // Live actor: the closing leg needs a successful command that really answers.
    let api = Arc::new(live_state(BootTimings::default()));
    let session = guard_capture_session().await;

    let flood = 3 * GUARD_TRACKED_PATHS_PER_CLASS;
    for index in 0..flood {
        let refused = control_router(Arc::clone(&api))
            .oneshot(
                Request::post(format!("/bogus/{index}"))
                    .header("host", "127.0.0.1:8708")
                    .header("origin", "http://evil.example")
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .expect("probe request"),
            )
            .await
            .expect("in-process probe");
        assert_eq!(
            refused.status(),
            StatusCode::FORBIDDEN,
            "a foreign Origin is refused before the token, on any path"
        );
        // The attacker's free re-arm packet: the public page answers 200 with
        // no token in every posture.
        let page = control_router(Arc::clone(&api))
            .oneshot(
                Request::get("/")
                    .header("host", "127.0.0.1:8708")
                    .body(Body::empty())
                    .expect("page request"),
            )
            .await
            .expect("in-process page read");
        assert_eq!(page.status(), StatusCode::OK);
    }
    let lines = session.stop();
    let origin_lines = lines
        .iter()
        .filter(|line| line.contains("guard refused") && line.contains("refusal=\"origin\""))
        .count();
    assert_eq!(
        origin_lines, GUARD_TRACKED_PATHS_PER_CLASS,
        "the attacker chose {flood} distinct paths and {flood} free page 200s; the \
             ring may still only pay for the tracked cap: {lines:?}"
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("guard refusal window full"))
            .count(),
        1,
        "the overflow collapses to the class's single fold line: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .all(|line| !line.contains("guard refusals cleared")),
        "a public page is not a command: no window may close and re-arm mid-flood: \
             {lines:?}"
    );
    assert_eq!(
        refused_lines(&lines),
        GUARD_TRACKED_PATHS_PER_CLASS,
        "the whole interleaved flood costs the cap, not one line per packet: \
             {lines:?}"
    );
    assert_eq!(
        api.guard_folds()
            .into_iter()
            .filter(|(class, _)| class == "origin")
            .collect::<Vec<_>>(),
        vec![(
            "origin".to_string(),
            (flood - GUARD_TRACKED_PATHS_PER_CLASS) as u64
        )],
        "the collapsed paths keep counting across the page interleaves: {:?}",
        api.guard_folds()
    );

    // A real command still closes the window exactly once and re-arms it.
    let recovery = guard_capture_session().await;
    let health = control_router(Arc::clone(&api))
        .oneshot(
            Request::get("/health")
                .header("host", "127.0.0.1:8708")
                .body(Body::empty())
                .expect("health request"),
        )
        .await
        .expect("in-process health read");
    assert_eq!(health.status(), StatusCode::OK);
    let closed = recovery.stop();
    let cleared = closed
        .iter()
        .filter(|line| line.contains("guard refusals cleared"))
        .collect::<Vec<_>>();
    assert_eq!(cleared.len(), 1, "exactly one summary line: {closed:?}");
    assert!(
        cleared[0].contains(&format!(
            "origin: +{} further paths folded",
            flood - GUARD_TRACKED_PATHS_PER_CLASS
        )),
        "the real command publishes the whole collapse the pages hid: {}",
        cleared[0]
    );
    assert!(api.guard_report().is_empty() && api.guard_folds().is_empty());
}

/// The `reason` field value of a captured `guard refused` line. The test
/// capture renders `%value` fields raw (unquoted) in BTreeMap name order, so
/// the value runs from `reason=` to the next field, ` refusal=`.
fn captured_reason(line: &str) -> &str {
    let start = line
        .find("reason=")
        .expect("the report line carries its reason")
        + "reason=".len();
    let rest = &line[start..];
    let end = rest
        .find(" refusal=")
        .expect("the reason field is followed by refusal");
    &rest[..end]
}

/// The logged refusal `reason` embedded the attacker's own path,
/// `Host` or `Origin` at full size — hyper's head bound alone lets one request
/// carry ~408 KiB of them into each of the 500 ring lines — while the
/// `GUARD_TRACKED_PATH_CHARS` doc says the line and the key are capped
/// together. Falsifier: pass `reason = %reason` uncapped and the length leg
/// fails with a several-hundred-char value on both classes. The client wire
/// body keeps the caller's own full path on purpose.
#[tokio::test]
async fn a_guard_refusal_line_carries_a_reason_capped_to_the_tracked_width() {
    crate::observability::set_extended_logging(false);
    const OFFERED: &str = "sekrit-never-in-the-ring";
    let api = Arc::new(ApiState::new(
        AppConfig {
            control_token: Some(OFFERED.to_string()),
            ..AppConfig::default()
        },
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    ));
    let session = guard_capture_session().await;

    // Token class: a kilobyte-long path refused 401 before route membership.
    let long_path = format!("/long/{}", "p".repeat(4 * GUARD_TRACKED_PATH_CHARS));
    let response = control_router(Arc::clone(&api))
        .oneshot(
            Request::post(long_path.as_str())
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .body(Body::empty())
                .expect("probe request"),
        )
        .await
        .expect("in-process probe");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("refusal body");
    let body = String::from_utf8_lossy(&body).into_owned();
    assert!(
        body.contains(&long_path),
        "the client wire body keeps the caller's own full path — only the \
             operator-facing line is capped: {body}"
    );

    // Host class: an oversized `Host` header on an ordinary path.
    let huge_host = format!(
        "{}.evil.test:8708",
        "h".repeat(4 * GUARD_TRACKED_PATH_CHARS)
    );
    let response = control_router(Arc::clone(&api))
        .oneshot(
            Request::post("/har/rescan")
                .header("host", huge_host.as_str())
                .header("content-type", "application/json")
                .body(Body::empty())
                .expect("probe request"),
        )
        .await
        .expect("in-process probe");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let lines = session.stop();
    let refused: Vec<&String> = lines
        .iter()
        .filter(|line| line.contains("guard refused"))
        .collect();
    assert_eq!(
        refused.len(),
        2,
        "one token line and one host line, both attacker-sized on the old code: \
             {lines:?}"
    );
    for line in &refused {
        let reason = captured_reason(line);
        assert!(
            reason.chars().count() <= GUARD_TRACKED_PATH_CHARS,
            "the ring/file `reason` field may not carry an attacker-sized value \
                 ({} chars): {reason}",
            reason.chars().count()
        );
        assert!(
            reason.ends_with("..."),
            "a hyper-sized input must show the elision, not a silent truncation: \
                 {reason}"
        );
    }
    let token_line = refused
        .iter()
        .find(|line| line.contains("refusal=\"token\""))
        .expect("the token class reports");
    assert!(
        captured_reason(token_line).starts_with("unauthorized:"),
        "the cap keeps the class prefix the operator reads: {token_line}"
    );
}

/// Structural refusals must be answered strictly before the
/// token check (README:265-268, DECISIONS LOCKED). A page that knows nothing about
/// the token must be told the security reason it triggered, not that a route wants
/// a secret — the 401 is an oracle for "this route exists and is protected".
/// Falsifier: move the `auth_reason` block above `reject_reason` in `guard` and all
/// three structural arms below fail with 401. The fourth arm is the same rule:
/// a request that carries no `Host` at all is itself a structural refusal and is
/// answered `400`, not allowed past the DNS-rebinding gate to the token.
#[tokio::test]
async fn structural_refusals_are_answered_before_the_token_is_consulted() {
    let config = AppConfig {
        control_token: Some("a-token-never-presented".to_string()),
        ..AppConfig::default()
    };
    let api = Arc::new(ApiState::new(
        config,
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    ));

    let structural = |builder: axum::http::request::Builder| async {
        control_router(Arc::clone(&api))
            .oneshot(builder.body(Body::empty()).expect("probe request"))
            .await
            .expect("in-process probe")
            .status()
    };
    assert_eq!(
        structural(Request::post("/har/rescan").header("host", "127.0.0.1:8708")).await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "a content-type probe with no token is a 415, never a 401"
    );
    assert_eq!(
        structural(
            Request::post("/har/rescan")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
                .header("origin", "http://evil.example")
        )
        .await,
        StatusCode::FORBIDDEN,
        "a cross-site POST with no token is a 403, never a 401"
    );
    assert_eq!(
        structural(
            Request::post("/har/rescan")
                .header("content-type", "application/json")
                .header("host", "rebind.example")
        )
        .await,
        StatusCode::BAD_REQUEST,
        "a DNS-rebinding Host with no token is a 400, never a 401"
    );
    assert_eq!(
        structural(
            Request::post("/har/rescan")
                .header("content-type", "application/json")
                .header("x-har-token", "a-token-never-presented")
        )
        .await,
        StatusCode::BAD_REQUEST,
        "the right token does not excuse a request the Host gate cannot inspect"
    );
    // And the token check is still there behind them: structurally clean + no
    // credential is the one case that may answer 401.
    assert_eq!(
        structural(
            Request::post("/har/rescan")
                .header("host", "127.0.0.1:8708")
                .header("content-type", "application/json")
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "with nothing structural to refuse, the token is what gates the route"
    );
}
