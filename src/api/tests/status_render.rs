use super::*;

/// The state pump builds the merged document once and every
/// SSE subscriber reads the same published value. Falsifier: restore the
/// per-client `merged_state` loop and the second subscriber's value is a
/// different build (and the pump flag is never set).
#[tokio::test]
async fn the_state_pump_builds_once_and_publishes_to_every_subscriber() {
    let state = Arc::new(live_state(BootTimings::default()));
    state.ensure_state_pump();
    let mut first = state.state_events.subscribe();
    let second = state.state_events.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while matches!(*first.borrow(), StateEvent::Empty) {
        tokio::time::timeout_at(deadline, first.changed())
            .await
            .expect("the pump publishes within 5s")
            .expect("the pump's sender is alive");
    }
    let doc = match &*first.borrow() {
        StateEvent::Doc(doc) => Arc::clone(doc),
        _ => panic!("the pump published a document"),
    };
    assert!(
        doc.get("models").is_some(),
        "the merged document carries the model table: {doc}"
    );
    assert!(
        matches!(*second.borrow(), StateEvent::Doc(_)),
        "a late subscriber reads the same published value, not a second build"
    );
    assert!(
        state.state_pump_started.load(Ordering::Acquire),
        "the one-shot pump flag is set"
    );
}

/// The loaded row's two new facts are server-computed
/// and **absent when not loaded** (never null — not-loaded is a state, not an
/// empty value), and the same-name merge is honest: earliest launch (fixed-width
/// UTC ISO sorts as it compares), any-of stale. Falsifier: emitting nulls for
/// unloaded rows (the page would read them as falsy-but-present) or a max/latest
/// merge of `launched_at` fails the respective leg.
#[test]
fn model_json_marks_loaded_rows_and_nobody_else() {
    let entry = ModelEntry::from_flat(
        &BTreeMap::from([("name".to_string(), "m".to_string())]),
        "m",
    )
    .expect("entry");
    let plain = model_json(&entry, None, false);
    assert!(
        plain.get("launched_at").is_none() && plain.get("stale_config").is_none(),
        "an unloaded row carries neither key, not even null: {plain}"
    );
    let loaded = LoadedRow {
        launched_at: "2026-10-29T03:55:12Z".to_string(),
        stale_config: true,
    };
    let body = model_json(&entry, Some(&loaded), false);
    assert_eq!(body["launched_at"], json!("2026-10-29T03:55:12Z"));
    assert_eq!(body["stale_config"], json!(true));

    let slot = |model: &str, launched: &str, stale: bool| crate::scheduler::SlotStatus {
        category: CategoryId::Work,
        index: 0,
        model: Some(model.to_string()),
        port: Some(1),
        pid: Some(1),
        need_gib: Some(4.0),
        idle_s: Some(0),
        serve_id: None,
        loading_job: None,
        state: "serving".to_string(),
        launched_at: Some(launched.to_string()),
        stale_config: Some(stale),
        container_reported_gb: None,
        booked_ram_gb: None,
        ram_cost_basis: None,
        ram_source: None,
        loaded_at: Instant::now(),
    };
    let rows = loaded_rows(&[
        slot("twin", "2026-05-05T00:00:00Z", false),
        slot("twin", "2026-01-01T00:00:00Z", true),
        slot("single", "2026-10-29T03:55:12Z", false),
    ]);
    assert_eq!(
        rows["twin"],
        LoadedRow {
            launched_at: "2026-01-01T00:00:00Z".to_string(),
            stale_config: true,
        },
        "same-name occupants merge: EARLIEST launch, ANY stale"
    );
    assert!(!rows["single"].stale_config, "clean slot, clean row");
    assert!(!rows.contains_key("ghost"));
}

/// The structural tier of the status-zero-stat rule (the repo's established
/// `include_str!` seam): the render/read bodies that publish `stale_config` and
/// `launched_at` may not reach for the filesystem or the clock — the verdict is
/// computed only at install, the reload turn, and harness write-backs.
///
/// The two-layer contract, stated honestly: the ban-list convicts SYNONYMS
/// (bare `fs::` catches `std::fs::` and `tokio::fs::`; `metadata(` catches
/// `std::fs::metadata`, `Path::metadata()` and `MetadataExt` spellings;
/// `SystemTime::now` is banned because the stamp is stored at the actor for
/// exactly that reason), inside these bodies only. What the net CANNOT see: a
/// stat whose result is discarded — inert by definition, it cannot move a
/// verdict — and a stat behind a helper not on the heads list. Anything whose
/// result REACHES the published verdict is convicted by the behavioural tier
/// too, regardless of spelling: the cache-probe in `scheduler`'s
/// `the_snapshot_reads_the_cached_verdict_and_never_stats` and the
/// `api_surface::config_writeback` hidden-file leg of
/// `a_flag_edit_never_ambers_its_own_loaded_model_and_status_never_stats`.
/// Tail sentinels assert each slice covers its body's end.
///
/// Falsifier: a `fs::`/`metadata(`/`fingerprint(`/`SystemTime::now` token
/// appearing in any scanned body trips this tripwire before a hot-path stall
/// can ship — including the synonym form that first slipped past the token list.
#[test]
fn the_status_and_models_render_paths_never_touch_the_filesystem() {
    let source = include_str!("../status_render.rs");
    for (head, tail) in [
        ("async fn render_status_document(", "\"config_errors\""),
        ("fn loaded_rows(", "or_insert"),
        ("fn model_json(", "stale_config"),
        ("async fn har_models(", "\"object\": \"list\""),
        ("async fn merged_state(", "Value::Array(models)"),
    ] {
        let start = source
            .find(head)
            .unwrap_or_else(|| panic!("{head} is still here"));
        let close = source[start..]
            .find("\n}\n")
            .unwrap_or_else(|| panic!("{head} closes at column 0"));
        let body = &source[start..start + close];
        assert!(
            body.contains(tail),
            "the scan slice must reach {head}'s tail (early-close fragility)"
        );
        for banned in [
            "fs::",
            "fingerprint(",
            "read_to_string(",
            "metadata(",
            "modified(",
            "try_exists",
            "File::",
            "FileTimes",
            "OpenOptions",
            "SystemTime::now",
            "read_dir",
        ] {
            assert!(
                !body.contains(banned),
                "`{head}` may only render cached scheduler/config state; found `{banned}`"
            );
        }
    }
}

#[test]
fn boot_stamp_is_first_write_wins() {
    // The two late measures are written by whoever finishes the
    // work, so the cell must admit exactly one writer. A plain store would let
    // the last supervisor to bind overwrite the first one's timestamp.
    let stamp = BootMsStamp::default();
    assert_eq!(
        stamp.get(),
        None,
        "an unstamped measure is absent, never reported as 0"
    );
    assert!(stamp.stamp(3), "the first writer wins");
    assert!(
        !stamp.stamp(9_999),
        "a later writer must be ignored, not overwrite the measure"
    );
    assert_eq!(stamp.get(), Some(3));
    let zero = BootMsStamp::default();
    assert!(zero.stamp(0), "a sub-millisecond event is still an event");
    assert_eq!(
        zero.get(),
        Some(0),
        "0 ms is a measurement, not the 'never happened' sentinel"
    );
    assert!(
        !zero.stamp(5),
        "and it still closes the cell to later writers"
    );
}

#[test]
fn concurrent_boot_stamps_admit_exactly_one_writer() {
    // The same rule under a real race: three supervisors released together, one
    // value lands and it is one of the three that were offered.
    let stamp = BootMsStamp::default();
    let barrier = std::sync::Barrier::new(3);
    let winners = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for ms in [11_u64, 22, 33] {
            let stamp = stamp.clone();
            let barrier = &barrier;
            let winners = &winners;
            scope.spawn(move || {
                barrier.wait();
                if stamp.stamp(ms) {
                    winners.lock().expect("winners lock").push(ms);
                }
            });
        }
    });
    let won = winners.lock().expect("winners lock").clone();
    assert_eq!(won.len(), 1, "exactly one writer may land; got {won:?}");
    assert_eq!(
        stamp.get(),
        Some(*won.first().expect("one winner")),
        "the value `/har/status.boot` reports is the winner's"
    );
}

#[tokio::test]
async fn status_boot_reports_the_five_measures_as_they_complete() {
    // `/har/status.boot` must carry the whole §0.2 measure set.
    // The three boot-thread fields are present from the start; the two that
    // complete after `ApiState` is shared are ABSENT (skipped, not null) until
    // stamped, then present with exactly the stamped value — the same shape
    // `bind_ms` has always had.
    let api = live_state(BootTimings {
        config_ms: 4,
        scheduler_spawn_ms: 6,
        ..BootTimings::default()
    });
    let before = status_document(&api).await.expect("status document");
    assert_eq!(before["boot"]["config_ms"], json!(4));
    assert_eq!(before["boot"]["scheduler_spawn_ms"], json!(6));
    assert!(
        before["boot"].get("bind_ms").is_none(),
        "bind_ms keeps its skip-when-unset shape: {before}"
    );
    assert!(
        before["boot"].get("first_bind_at").is_none(),
        "an unstamped first bind is absent from the document: {before}"
    );
    assert!(
        before["boot"].get("reap_ms").is_none(),
        "an unstamped reclaim is absent from the document: {before}"
    );

    api.first_bind_stamp().stamp(1);
    api.reap_stamp().stamp(4_321);
    api.boot.write().expect("boot timings lock").set_bound(20);

    let after = status_document(&api).await.expect("status document");
    assert_eq!(after["boot"]["first_bind_at"], json!(1), "ms-since-start");
    assert_eq!(after["boot"]["reap_ms"], json!(4_321));
    assert_eq!(after["boot"]["bind_ms"], json!(20));
    assert!(
        !api.first_bind_stamp().stamp(500),
        "the shared cell behind the lock is the one the route serializes"
    );
    let again = status_document(&api).await.expect("status document");
    assert_eq!(
        again["boot"]["first_bind_at"],
        json!(1),
        "and it is still first-write-wins through the API handle"
    );
}

/// The reconcile rider on `/har/status`: the warned names appear when the
/// snapshot reports them and the key is ABSENT otherwise — absent, not null,
/// not `[]` — so a harness with nothing stranded sends the exact old document
/// (the fixture-parity pin keeps the DOM suite honest about that shape).
#[tokio::test]
async fn status_document_carries_stranded_containers_only_when_reported() {
    let api = live_state(BootTimings::default());
    let base = api.scheduler.status().await.expect("actor alive");
    let doc = render_status_document(&api, &base).await;
    assert!(
        doc.get("stranded_containers").is_none(),
        "no report: the key is absent, not null: {doc}"
    );
    let mut reported = base.clone();
    reported.stranded_containers = Some(["r5-api-a".to_string()].into_iter().collect());
    let doc = render_status_document(&api, &reported).await;
    assert_eq!(
        doc["stranded_containers"],
        json!(["r5-api-a"]),
        "the reconcile's warned names ride /har/status"
    );
    let mut emptied = base.clone();
    emptied.stranded_containers = Some(std::collections::BTreeSet::new());
    let doc = render_status_document(&api, &emptied).await;
    assert!(
        doc.get("stranded_containers").is_none(),
        "an empty set never reaches the page as []: {doc}"
    );
}

/// Every `/har/status` category row carries an
/// additive `label` — the trimmed `label`, or the id when it is absent or
/// blank — always present, beside the unchanged `id` and every pre-existing
/// key. The label is display-only; the id stays the wire vocabulary.
/// Falsifier: return an empty string from the `effective_label` fallback and
/// the absent/blank legs go red; drop the always-set in `render_status_document`
/// and the set-but-blank row loses its key.
#[tokio::test]
async fn status_categories_carry_the_effective_label_beside_the_id() {
    let mut config = AppConfig::default();
    config.categories[1].label = Some("Theater".to_string());
    config.categories[2].label = Some("".to_string());
    let api = ApiState::new(
        config,
        SchedulerHandle::test_dummy(),
        crate::listeners::ListenerRegistry::new(),
        Arc::new(crate::budget::LedgerOnlyWitness),
        BootTimings::default(),
    );
    let empty = SchedulerStatus {
        slots: Vec::new(),
        jobs: Vec::new(),
        catalog_errors: Vec::new(),
        stranded_containers: None,
        fit_validated: Default::default(),
    };
    let doc = render_status_document(&api, &empty).await;
    let categories = doc["categories"].as_array().expect("categories array");
    assert_eq!(categories.len(), 4);
    for row in categories {
        let id = row["id"].as_str().expect("the id stays the row identity");
        let label = row["label"]
            .as_str()
            .unwrap_or_else(|| panic!("`label` is always present, never absent or null: {row}"));
        let expected = if id == "RP" { "Theater" } else { id };
        assert_eq!(
            label, expected,
            "the effective label (trimmed `label`, else the id) rides beside the id: {row}"
        );
        for key in [
            "public_port",
            "max_models",
            "evictable_by",
            "engine_ports",
            "control",
        ] {
            assert!(
                row.get(key).is_some(),
                "the pre-existing key `{key}` stays: {row}"
            );
        }
    }
}

/// `/har/status` publishes the unsafe-but-legal postures —
/// `config_warnings` always rides as an array of `{code, message}` objects.
/// `0.0.0.0` without a token names `lan_open_without_token` and a resolved
/// token retires it; an unset `llama_server_exe` names `llama_server_exe_unset`
/// and a configured binary retires it; a sane posture warns about nothing (the
/// exact sentences are pinned where they are minted, `config::config_warnings`).
/// Falsifier: drop the `config_warnings` line in `render_status_document` and
/// every leg goes red; drop the token leg of the LAN condition and a guarded
/// harness starts nagging.
#[tokio::test]
async fn status_document_carries_config_warnings_for_unsafe_postures() {
    async fn warnings(config: AppConfig) -> Vec<serde_json::Value> {
        let api = ApiState::new(
            config,
            SchedulerHandle::test_dummy(),
            crate::listeners::ListenerRegistry::new(),
            Arc::new(crate::budget::LedgerOnlyWitness),
            BootTimings::default(),
        );
        let empty = SchedulerStatus {
            slots: Vec::new(),
            jobs: Vec::new(),
            catalog_errors: Vec::new(),
            stranded_containers: None,
            fit_validated: Default::default(),
        };
        let doc = render_status_document(&api, &empty).await;
        doc["config_warnings"]
            .as_array()
            .expect("`config_warnings` is always an array")
            .clone()
    }
    let has = |warnings: &[serde_json::Value], code: &str| {
        warnings
            .iter()
            .any(|w| w["code"].as_str() == Some(code) && !w["message"].as_str().unwrap().is_empty())
    };

    let default = warnings(AppConfig::default()).await;
    assert!(
        has(&default, "llama_server_exe_unset"),
        "an unset llama binary is a posture the page must show: {default:?}"
    );
    assert!(
        !has(&default, "lan_open_without_token"),
        "a loopback boot owes no LAN warning: {default:?}"
    );

    let published = warnings(AppConfig {
        internal_host: "0.0.0.0".parse().expect("publish bind"),
        ..AppConfig::default()
    })
    .await;
    assert!(
        has(&published, "lan_open_without_token"),
        "0.0.0.0 with no token must ride every status read: {published:?}"
    );

    let guarded = warnings(AppConfig {
        internal_host: "0.0.0.0".parse().expect("publish bind"),
        control_token: Some("a-token-never-elsewhere".to_string()),
        ..AppConfig::default()
    })
    .await;
    assert!(
        !has(&guarded, "lan_open_without_token"),
        "a resolved token retires the LAN warning: {guarded:?}"
    );

    let configured = warnings(AppConfig {
        llama_server_exe: Some(std::path::PathBuf::from("D:/llama/llama-server.exe")),
        ..AppConfig::default()
    })
    .await;
    assert!(
        configured.is_empty(),
        "a loopback boot with a configured binary warns about nothing: {configured:?}"
    );
}

/// The "A + amber" verdict policy: a category whose
/// supervisor never spoke must be PRESENT in the `listeners` object and LABELED
/// `Pending`, so the page can never read "no verdict" as "no such category" (the
/// old shape made the two indistinguishable). Falsifier: return the registry to an
/// unseeded `new()` and the four `Pending` assertions fail on a missing key.
#[tokio::test]
async fn a_category_with_no_verdict_is_present_and_labeled_pending() {
    let api = live_state(BootTimings::default());
    let fresh = status_document(&api).await.expect("status document");
    assert_eq!(
        fresh["listeners"].as_object().map(|m| m.len()),
        Some(4),
        "the passthrough serializes every configured category, not only the marked ones: {fresh}"
    );
    for name in ["WORK", "RP", "AUX", "EMB"] {
        assert_eq!(
            fresh["listeners"][name],
            json!("Pending"),
            "a never-marked category is present AND labeled: {fresh}"
        );
    }

    api.listeners.mark_serving(CategoryId::Work);
    api.listeners.mark_down(
        CategoryId::Emb,
        "cannot bind 127.0.0.1:8711: address in use".to_string(),
    );
    let after = status_document(&api).await.expect("status document");
    assert_eq!(after["listeners"]["WORK"], json!("Serving"));
    assert_eq!(
        after["listeners"]["RP"],
        json!("Pending"),
        "neighbours keep their no-verdict label while the boot is incomplete"
    );
    assert_eq!(
        after["listeners"]["EMB"]["Down"]["reason"],
        json!("cannot bind 127.0.0.1:8711: address in use"),
        "the four verdicts stay three distinct shapes"
    );
    assert!(
        after["listeners"].get("AUX").is_some(),
        "a category nobody marked is never the same as a removed one: {after}"
    );
    assert!(
        after["listeners"].get("NOSUCH").is_none(),
        "an absent category is a MISSING key — which seeding makes unrepresentable"
    );
}

/// The two `/har/status` booleans the header badge reads, pinned as exact JSON for
/// each of the three shapes that distinguish them: four times
/// `Serving`, one `Down`, one `Pending`. Every field carries both a true and a
/// false leg, so neither verdict can be hardcoded in the other's place.
///
/// # Falsifier
///
/// Swap the two sources (`listeners_down` from `pending()`, or both from
/// `any_down()`) and two of the three fixtures fail the exact compare; drop either
/// field and it reads `null` against `false`/`true` and fails too — the page has no
/// fallback able to absorb that, because `renderHeader` reads these booleans and no
/// longer decodes `listeners` at all (pinned by
/// `served_app_js_reads_the_server_computed_listener_verdicts`).
#[tokio::test]
async fn status_document_emits_the_two_listener_verdicts_exactly() {
    fn listener_verdicts(doc: &Value) -> Value {
        json!({
            "listeners_down": doc["listeners_down"].clone(),
            "listeners_pending": doc["listeners_pending"].clone(),
        })
    }

    let all_serving = live_state(BootTimings::default());
    for category in CategoryId::all() {
        all_serving.listeners.mark_serving(category);
    }
    let doc = status_document(&all_serving)
        .await
        .expect("status document");
    assert_eq!(
        listener_verdicts(&doc),
        json!({ "listeners_down": false, "listeners_pending": false }),
        "four times Serving is the only all-clear: {doc}"
    );

    let one_down = live_state(BootTimings::default());
    for category in CategoryId::all() {
        one_down.listeners.mark_serving(category);
    }
    one_down
        .listeners
        .mark_down(CategoryId::Aux, "address in use".to_string());
    let doc = status_document(&one_down).await.expect("status document");
    assert_eq!(
        listener_verdicts(&doc),
        json!({ "listeners_down": true, "listeners_pending": false }),
        "one Down is red, and a Down is a verdict — never also a pending: {doc}"
    );

    let one_pending = live_state(BootTimings::default());
    for category in [CategoryId::Work, CategoryId::Rp, CategoryId::Emb] {
        one_pending.listeners.mark_serving(category);
    }
    let doc = status_document(&one_pending)
        .await
        .expect("status document");
    assert_eq!(
        listener_verdicts(&doc),
        json!({ "listeners_down": false, "listeners_pending": true }),
        "the category nobody has spoken for is pending, not down: {doc}"
    );
}

/// Reading the ring must not append to the ring, or the act of
/// asking "why did the last load fail" evicts the answer. Extended to every member
/// of the routine list — `/har/models` is polled on the same
/// 2 s timer and was silently missing from the exemption, so the models panel was
/// the one poll still evicting evidence. Falsifier: drop `/har/log` (or any member)
/// from `ROUTINE_POLL_PATHS` and the zero-count assertion fails with that path's
/// `request complete` lines present.
#[tokio::test]
async fn reading_the_log_never_appends_to_the_log() {
    // The switch is process-global and off at boot; state it explicitly so this
    // pin means "exempted", not "someone turned the logging off".
    crate::observability::set_extended_logging(false);
    // A live actor, not `unit_state`: a routine poll that 500s would make the
    // exemption pass for the wrong reason (the guard only mutes 2xx polls).
    let api = Arc::new(live_state(BootTimings::default()));
    // `request complete` is its own callsite (`:1465`, not the guard's) and the
    // assertion below reads its FIRST emission, so the same process-wide interest
    // hazard that made the guard pins flaky (D9) applies here: prove it delivers
    // into a capture — on a throwaway state — before the measured window opens.
    warm_capture_lines(&["request complete"], || async {
        let warm = warm_state(None);
        let response = control_router(Arc::clone(&warm))
            .oneshot(
                Request::get("/health")
                    .header("host", "127.0.0.1:8708")
                    .body(Body::empty())
                    .expect("warm-up request"),
            )
            .await
            .expect("in-process warm-up");
        assert_eq!(response.status(), StatusCode::OK);
    })
    .await;
    let session = crate::tray::testlog::Session::start();
    let polled = pages_own_polls();
    for path in &polled {
        for _ in 0..2 {
            let response = control_router(Arc::clone(&api))
                .oneshot(
                    Request::get(path)
                        .header("host", "127.0.0.1:8708")
                        .body(Body::empty())
                        .expect("routine poll request"),
                )
                .await
                .expect("in-process routine poll");
            assert_eq!(response.status(), StatusCode::OK, "{path} must answer");
        }
    }
    // And a GET that is not a routine poll still reports, so the exemption is
    // scoped rather than an accidental mute of the whole request line.
    let other = control_router(Arc::clone(&api))
        .oneshot(
            Request::get("/health")
                .header("host", "127.0.0.1:8708")
                .body(Body::empty())
                .expect("health request"),
        )
        .await
        .expect("in-process health read");
    assert_eq!(other.status(), StatusCode::OK, "/health must answer");
    let lines = session.stop();
    for path in &polled {
        let poll_lines = lines
            .iter()
            .filter(|line| line.contains("request complete") && line.contains(path))
            .count();
        assert_eq!(
            poll_lines, 0,
            "a poll the page runs every 2 s may not evict the ring it is read from: \
                 {path} logged {poll_lines} lines: {lines:?}"
        );
    }
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("request complete") && line.contains("/health"))
            .count(),
        1,
        "non-routine GETs stay in the ring (the exemption is scoped): {lines:?}"
    );
}

/// The routes `webui/app.js` actually fetches with `getJson(...)`, query stripped.
/// The behavior pin polls THIS list and the parity pin compares it to the const, so
/// shrinking `ROUTINE_POLL_PATHS` fails both: the const cannot quietly stop covering
/// a poll the page still runs.
fn pages_own_polls() -> Vec<String> {
    let page = include_str!("../../webui/app.js");
    let mut polled: Vec<String> = Vec::new();
    for rest in page
        .match_indices("getJson(\"")
        .map(|(index, _)| &page[index + 9..])
    {
        if let Some(end) = rest.find('"') {
            let literal = &rest[..end];
            // The page's log panel asks for a window, not the bare route; the guard
            // matches on `uri().path()`, so the query is not part of the identity.
            let route = literal.split('?').next().unwrap_or(literal).to_string();
            if route.starts_with("/har/") && !polled.contains(&route) {
                polled.push(route);
            }
        }
    }
    polled.sort();
    polled
}

/// The exemption list and the page's own polling code were two
/// files that nothing linked, so the list could shrink to "everything except
/// /har/log" and stay green. The third source of truth is `webui/app.js` itself:
/// every `getJson(...)` literal the page fetches on its timer must be in
/// `ROUTINE_POLL_PATHS`, and nothing extra may be (an exempt path is a path whose
/// requests leave no trace — a mute button needs proving absent, not assumed).
/// Falsifier: remove `"/har/models"` from the const and the set equality fails.
#[test]
fn the_routine_poll_list_is_the_pages_own_poll_list() {
    let polled = pages_own_polls();
    let mut declared: Vec<String> = ROUTINE_POLL_PATHS
        .iter()
        .map(|p| (*p).to_string())
        .collect();
    declared.sort();
    assert_eq!(
        declared, polled,
        "`ROUTINE_POLL_PATHS` drifted from the page's own `getJson(...)` literals \
             (a poll that is not exempted evicts the ring; an exempted route nobody \
             polls hides its traffic)"
    );
}

/// A `limit` the handler cannot parse is a request-shape refusal —
/// `400 bad_request` in the project dual envelope (flat + OpenAI `error`),
/// never the default-100 the old `.ok()` silently swallowed it into, and
/// never axum text/plain (the project envelope rule). The malformed legs:
/// `abc`, empty, negative. Falsifiers: restore `.parse().ok().unwrap_or(100)`
/// and the 400 legs answer 200; answer `text/plain` and the content-type and
/// envelope legs fail; the `%zz` leg (no `limit` key at all) must STAY 200 —
/// lenient decoding is not a limit error.
#[tokio::test]
async fn a_malformed_log_limit_answers_400_in_the_project_envelope() {
    let api = Arc::new(unit_state());
    for query in ["limit=abc", "limit=", "limit=-1"] {
        let response = control_router(Arc::clone(&api))
            .oneshot(
                Request::get(format!("/har/log?{query}"))
                    .header("host", "127.0.0.1:8708")
                    .body(Body::empty())
                    .expect("log request"),
            )
            .await
            .expect("in-process log request");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "`{query}` must be refused, not swallowed"
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default(),
            "application/json",
            "the refusal answers the project envelope, never text/plain"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("refusal body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json envelope");
        assert_eq!(body["code"], serde_json::json!("bad_request"), "{body}");
        assert_eq!(
            body["error"]["code"],
            serde_json::json!("bad_request"),
            "the OpenAI sub-object carries the same class: {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("limit"),
            "the refusal names the refused parameter: {message}"
        );
        if query == "limit=abc" {
            assert!(
                message.contains("abc"),
                "the refusal quotes the malformed value back: {message}"
            );
        }
    }
    // The lenient-decode legs that are NOT limit errors stay inside the ring
    // document (the envelope posture for inputs without a `limit` key).
    let response = control_router(api)
        .oneshot(
            Request::get("/har/log?%zz")
                .header("host", "127.0.0.1:8708")
                .body(Body::empty())
                .expect("log request"),
        )
        .await
        .expect("in-process log request");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a query with no limit key still answers the ring document"
    );
}

/// The limit rule, the other leg: a valid `limit` still answers the ring
/// document and
/// still clamps to the ring capacity. `limit=2` returns exactly the newest 2
/// events; `limit=999999` is accepted (not a refusal) and capped by
/// `RING_CAPACITY` — the events list can never exceed capacity. Falsifiers:
/// the new 400 arm rejecting a valid number fails the 200 legs; dropping the
/// `.min(RING_CAPACITY)` is invisible in shape but the `limit=2` leg pins the
/// clamp itself (a list longer than 2 here fails).
#[tokio::test]
async fn a_valid_log_limit_still_clamps_to_the_ring_capacity() {
    let api = Arc::new(unit_state());
    for index in 0..3 {
        crate::observability::ring_push(crate::observability::LogEntry::now(
            "INFO",
            "test::har_log_clamp",
            format!("fr05-clamp-marker-{index}"),
        ));
    }
    async fn get_log(api: &Arc<ApiState>, query: &str) -> (StatusCode, serde_json::Value) {
        let response = control_router(Arc::clone(api))
            .oneshot(
                Request::get(format!("/har/log?{query}"))
                    .header("host", "127.0.0.1:8708")
                    .body(Body::empty())
                    .expect("log request"),
            )
            .await
            .expect("in-process log request");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("log body");
        let body = serde_json::from_slice(&bytes).expect("ring document");
        (status, body)
    }
    let (status, body) = get_log(&api, "limit=2").await;
    assert_eq!(status, StatusCode::OK, "a valid limit is never refused");
    let events = body["events"].as_array().expect("events list");
    assert_eq!(
        events.len(),
        2,
        "limit=2 returns exactly the newest 2: {body}"
    );
    assert!(
        events[1]["message"]
            .as_str()
            .is_some_and(|m| m == "fr05-clamp-marker-2"),
        "the tail is the newest event: {body}"
    );
    let (status, body) = get_log(&api, "limit=999999").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a huge limit is clamped, not refused"
    );
    let events = body["events"].as_array().expect("events list");
    assert_eq!(
        body["capacity"],
        serde_json::json!(crate::observability::RING_CAPACITY)
    );
    assert!(
        events.len() <= crate::observability::RING_CAPACITY,
        "the answer can never exceed the ring capacity"
    );
}
