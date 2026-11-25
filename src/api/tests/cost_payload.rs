use super::*;

#[test]
fn model_json_cpu_offload_agrees_with_the_engine() {
    // Review finding 6: one truth set — engine/llama::jinja_on — so the page
    // checkbox can never disagree with whether `-ngl 0` was actually forced.
    let entry = |cpu: &str| ModelEntry {
        name: "m".to_string(),
        engine: "llama".to_string(),
        category: CategoryId::Work,
        vram_gb: Some(4.0),
        ram_gb: None,
        gguf_size_gb: None,
        cost: crate::model::CostClass::Fixed,
        vram_gate: crate::model::VramGate::default(),
        load_timeout_s: None,
        min_turn_s: None,
        measured_gb: None,
        measured_min_gb: None,
        measured_offload: None,
        measured_attribution: None,
        measured_profile: None,
        measured_ram_gb: None,
        measured_ram_min_gb: None,
        fit_gb: None,
        model_ref: Some(r"F:\m.gguf".to_string()),
        serve_model: None,
        file: None,
        raw: BTreeMap::from([
            ("cpu_offload".to_string(), cpu.to_string()),
            ("context".to_string(), "262144".to_string()),
        ]),
    };
    for truthy in ["true", "1", "on"] {
        assert_eq!(
            model_json(&entry(truthy), None, false)["cpu_offload"],
            json!(true),
            "`{truthy}` is engine-truthy"
        );
    }
    assert_eq!(
        model_json(&entry("yes"), None, false)["cpu_offload"],
        json!(false)
    );
    assert_eq!(
        model_json(&entry("false"), None, false)["cpu_offload"],
        json!(false)
    );
    let body = model_json(&entry("true"), None, false);
    assert_eq!(body["context"], json!("262144"));
    assert_eq!(body["model_ref"], json!(r"F:\m.gguf"));
    assert_eq!(body["cost"], json!("fixed"));
    assert!(body["launch_floor_gb"].is_null());
    assert!(body["load_timeout_s"].is_null());
    assert!(body["measured_gb"].is_null());
    assert!(
        body.get("fit_gb").is_none(),
        "a never-probed row has no fit key at all: absent is not null, and the
             fixture parity rides it"
    );
}

/// The page reads the gate's answer — `cost_basis: "fit"` only when
/// the projection actually books, and the `fit_gb` figure rides the payload
/// whenever it exists (even under a declared floor that keeps booking).
#[test]
fn the_fit_figure_rides_the_payload_only_as_the_gate_decides() {
    let row = |fit: Option<f64>| ModelEntry {
        fit_gb: fit,
        ..ModelEntry {
            name: "m".to_string(),
            engine: "llama".to_string(),
            category: CategoryId::Work,
            vram_gb: Some(24.0),
            ram_gb: None,
            gguf_size_gb: None,
            cost: crate::model::CostClass::Fixed,
            vram_gate: crate::model::VramGate::default(),
            load_timeout_s: None,
            min_turn_s: None,
            measured_gb: None,
            measured_min_gb: None,
            measured_offload: None,
            measured_attribution: None,
            measured_profile: None,
            measured_ram_gb: None,
            measured_ram_min_gb: None,
            fit_gb: None,
            model_ref: None,
            serve_model: None,
            file: None,
            raw: BTreeMap::new(),
        }
    };
    let books = model_json(&row(Some(31.0)), None, false);
    assert_eq!(books["cost_basis"], json!("fit"));
    assert_eq!(books["fit_gb"], json!(31.0));
    let floored = model_json(&row(Some(20.0)), None, false);
    assert_eq!(
        floored["cost_basis"],
        json!("declared"),
        "declared is a floor: a smaller projection displays but does not book"
    );
    assert_eq!(
        floored["fit_gb"],
        json!(20.0),
        "the figure itself still rides the payload for the page"
    );
}

/// Session cross-check, wire shape: `fit_validated` appears ONLY as a
/// `true` alongside a row the gate books from Fit — never null, never
/// `false`, and never beside a row the gate books some other way (the
/// verdict is about a Fit booking, so a row with no Fit booking cannot
/// carry it even if the set still names the model). Falsifier: emitting
/// the flag unconditionally fails the absent legs; dropping the cost-basis
/// gate fails the declared leg.
#[test]
fn the_fit_validated_key_rides_only_a_fit_booked_row_the_session_validated() {
    let fit_row = |fit: Option<f64>, declared: Option<f64>| ModelEntry {
        fit_gb: fit,
        vram_gb: declared,
        ram_gb: None,
        ..ModelEntry {
            name: "m".to_string(),
            engine: "llama".to_string(),
            category: CategoryId::Work,
            vram_gb: None,
            ram_gb: None,
            gguf_size_gb: None,
            cost: crate::model::CostClass::Fixed,
            vram_gate: crate::model::VramGate::default(),
            load_timeout_s: None,
            min_turn_s: None,
            measured_gb: None,
            measured_min_gb: None,
            measured_offload: None,
            measured_attribution: None,
            measured_profile: None,
            measured_ram_gb: None,
            measured_ram_min_gb: None,
            fit_gb: None,
            model_ref: None,
            serve_model: None,
            file: None,
            raw: BTreeMap::new(),
        }
    };
    let validated = model_json(&fit_row(Some(31.0), None), None, true);
    assert_eq!(validated["cost_basis"], json!("fit"));
    assert_eq!(
        validated["fit_validated"],
        json!(true),
        "a session-validated Fit row carries the flag"
    );
    let unvalidated = model_json(&fit_row(Some(31.0), None), None, false);
    assert!(
        unvalidated.get("fit_validated").is_none(),
        "an unvalidated row keeps the absent key, not null: {unvalidated}"
    );
    let declared = model_json(&fit_row(Some(20.0), Some(24.0)), None, true);
    assert!(
        declared.get("fit_validated").is_none(),
        "a row the gate books from its declared floor carries no verdict about a \
             Fit booking that is not happening: {declared}"
    );
    let unprobed = model_json(&fit_row(None, None), None, true);
    assert!(
        unprobed.get("fit_validated").is_none() && unprobed.get("fit_gb").is_none(),
        "a never-probed row is byte-identical to its pre-R11 shape: {unprobed}"
    );
}

#[test]
fn est_vram_gb_only_when_declared_and_measured_are_absent() {
    let mut entry = ModelEntry {
        name: "m".to_string(),
        engine: "llama".to_string(),
        category: CategoryId::Work,
        vram_gb: None,
        ram_gb: None,
        gguf_size_gb: Some(23.0),
        cost: crate::model::CostClass::Fixed,
        vram_gate: crate::model::VramGate::default(),
        load_timeout_s: None,
        min_turn_s: None,
        measured_gb: None,
        measured_min_gb: None,
        measured_offload: None,
        measured_attribution: None,
        measured_profile: None,
        measured_ram_gb: None,
        measured_ram_min_gb: None,
        fit_gb: None,
        model_ref: Some(r"F:\m.gguf".to_string()),
        serve_model: None,
        file: None,
        raw: BTreeMap::new(),
    };
    let body = model_json(&entry, None, false);
    assert_eq!(body["est_vram_gb"], json!(23.0 * 1.15 + 2.0));
    entry.measured_gb = Some(70.5);
    let body = model_json(&entry, None, false);
    assert_eq!(body["measured_gb"], json!(70.5));
    assert!(
        body["est_vram_gb"].is_null(),
        "admission books the measurement, not the estimate"
    );
}

/// RAM cost surface on `/har/models`: a cpu_offload row is priced on the
/// RAM ladder — `cost_basis` names the booked RAM rung in the same vocabulary,
/// the RAM figures ride, and `ram_source` says whose number the booking is —
/// while a GPU row keeps its exact pre-RAM shape: no RAM key at all, VRAM
/// ladder unchanged. The routing follows the row's resource, never which keys
/// happen to carry numbers: a RAM row holding a leftover card measurement
/// pair still reports the RAM rung, and an inert `ram_gb` on a card row
/// reports nothing. Falsifier: route `cost_basis` off `cost_source` for every
/// row and the leftover-pair leg reads `measured` (a card rung a CPU row never
/// books); drop the resource gate and the GPU row gains the RAM keys.
#[test]
fn the_cpu_models_row_prices_on_the_ram_ladder_and_a_gpu_row_stays_vram() {
    let entry = |yaml: &str, name: &str| -> ModelEntry {
        let (map, _) = crate::model::scan::parse_flat_yaml(yaml);
        ModelEntry::from_flat(&map, name).expect("entry")
    };
    let cpu_yaml = "engine: llama\ncpu_offload: true\nram_gb: 2\n";

    // Declared floor: the floor's own words, no estimate key.
    let body = model_json(&entry(cpu_yaml, "m"), None, false);
    assert_eq!(body["cost_basis"], json!("declared"));
    assert_eq!(body["ram_gb"], json!(2.0));
    assert!(body["measured_ram_gb"].is_null());
    assert!(body["measured_ram_min_gb"].is_null());
    assert_eq!(body["ram_source"], json!("declared"));
    assert!(
        body.get("est_ram_gb").is_none(),
        "the estimate key appears only when the estimate books: {body}"
    );

    // Measured rung: the trusted tree pair overshadows the floor and says
    // whose number it is.
    let mut measured = entry(cpu_yaml, "m");
    measured.measured_ram_gb = Some(9.4);
    measured.measured_ram_min_gb = Some(9.4);
    let body = model_json(&measured, None, false);
    assert_eq!(body["cost_basis"], json!("measured"));
    assert_eq!(body["measured_ram_gb"], json!(9.4));
    assert_eq!(body["measured_ram_min_gb"], json!(9.4));
    assert_eq!(body["ram_source"], json!("per_process"));

    // Estimate rung: no declaration and no pair — the GGUF estimate books,
    // and the estimate key shows exactly the booked figure.
    let mut estimate = entry("engine: llama\ncpu_offload: true\n", "m");
    estimate.gguf_size_gb = Some(2.0);
    let body = model_json(&estimate, None, false);
    assert_eq!(body["cost_basis"], json!("estimate"));
    assert_eq!(body["est_ram_gb"], json!(4.0));
    assert!(body["ram_gb"].is_null());
    assert_eq!(body["ram_source"], json!("declared"));

    // Routing: a leftover CARD measurement pair never pulls a CPU row onto a
    // card rung — the RAM basis answers for the resource the row books.
    let mut leftover = entry(cpu_yaml, "m");
    leftover.measured_gb = Some(30.0);
    leftover.measured_attribution = Some(crate::model::MeasuredAttribution::PerProcess);
    let body = model_json(&leftover, None, false);
    assert_eq!(
        body["cost_basis"],
        json!("declared"),
        "the card pair is a display leftover on a RAM row, never its basis: {body}"
    );

    // GPU twins: byte-shape of the pre-RAM row — no RAM key, VRAM ladder.
    let body = model_json(&entry("engine: llama\nvram_gb: 24\n", "g"), None, false);
    assert_eq!(body["cost_basis"], json!("declared"));
    for key in [
        "ram_gb",
        "est_ram_gb",
        "measured_ram_gb",
        "measured_ram_min_gb",
        "ram_source",
    ] {
        assert!(
            body.get(key).is_none(),
            "a GPU row carries no RAM key at all: {key} in {body}"
        );
    }
    let inert = entry("engine: mock\nvram_gb: 2\nram_gb: 8\n", "i");
    let body = model_json(&inert, None, false);
    assert_eq!(body["cost_basis"], json!("declared"));
    assert!(
        body.get("ram_gb").is_none() && body.get("ram_source").is_none(),
        "an inert `ram_gb` on a card row surfaces no RAM surface: {body}"
    );
}

/// RAM booking on the `/har/status` slot rows: every loaded CPU row
/// carries its booked GiB plus the rung and source labels (the exact wire
/// vocabulary), and a GPU row carries NONE of the three keys — the absent-not-
/// null rule every loaded-row field obeys, so GPU rows keep their pre-RAM
/// shape byte for byte. Falsifier: drop any `skip_serializing_if` and the GPU
/// leg reads `null` against absent.
#[tokio::test]
async fn a_cpu_status_slot_carries_its_ram_booking_and_a_gpu_slot_carries_none() {
    let api = live_state(BootTimings::default());
    let base = api.scheduler.status().await.expect("actor alive");
    let now = Instant::now();
    let ram_slot = |model: &str,
                    booked: Option<f64>,
                    basis: Option<&'static str>,
                    source: Option<&'static str>| SlotStatus {
        booked_ram_gb: booked,
        ram_cost_basis: basis,
        ram_source: source,
        ..occupied_slot(model, 9_200, now)
    };
    let status = SchedulerStatus {
        slots: vec![
            ram_slot("cpu-m", Some(9.4), Some("measured"), Some("per_process")),
            ram_slot("cpu-d", Some(8.0), Some("declared"), Some("declared")),
            ram_slot("cpu-e", Some(4.0), Some("estimate"), Some("declared")),
            occupied_slot("gpu-a", 9_201, now),
        ],
        ..base.clone()
    };
    let doc = render_status_document(&api, &status).await;
    let slots = doc["slots"].as_array().expect("slots array");
    assert_eq!(slots[0]["booked_ram_gb"], json!(9.4));
    assert_eq!(slots[0]["ram_cost_basis"], json!("measured"));
    assert_eq!(slots[0]["ram_source"], json!("per_process"));
    assert_eq!(slots[1]["booked_ram_gb"], json!(8.0));
    assert_eq!(slots[1]["ram_cost_basis"], json!("declared"));
    assert_eq!(slots[1]["ram_source"], json!("declared"));
    assert_eq!(slots[2]["booked_ram_gb"], json!(4.0));
    assert_eq!(slots[2]["ram_cost_basis"], json!("estimate"));
    assert_eq!(slots[2]["ram_source"], json!("declared"));
    for key in ["booked_ram_gb", "ram_cost_basis", "ram_source"] {
        assert!(
            slots[3].get(key).is_none(),
            "the GPU row shape predates the RAM surface: {key} in {}",
            slots[3]
        );
    }
    assert_eq!(slots[3]["need_gib"], json!(4.0), "and its card fields stay");
}

/// The `mem` block's RAM side: `booked_ram_gb` sums exactly the loaded CPU
/// rows' bookings (a GPU row never enters it, an empty host is a 0.0 sum not
/// an absence) and `ram_reserve_gb` names the reserve the RAM gate subtracts
/// before comparing, beside the pre-existing host reading. Falsifier: sum
/// from the slot table
/// without the `booked_ram_gb` filter and the GPU row's card figure lands in
/// the sum; omit the reserve key and the leg reads null.
#[tokio::test]
async fn the_mem_block_books_ram_only_where_rows_book_ram() {
    let api = live_state(BootTimings::default());
    let base = api.scheduler.status().await.expect("actor alive");
    let now = Instant::now();
    let ram_slot = |model: &str, booked: Option<f64>| SlotStatus {
        booked_ram_gb: booked,
        ram_cost_basis: booked.map(|_| "measured"),
        ram_source: booked.map(|_| "per_process"),
        ..occupied_slot(model, 9_300, now)
    };
    let mixed = SchedulerStatus {
        slots: vec![
            ram_slot("cpu-a", Some(9.4)),
            ram_slot("cpu-b", Some(5.6)),
            occupied_slot("gpu-a", 9_301, now),
        ],
        ..base.clone()
    };
    let doc = render_status_document(&api, &mixed).await;
    assert_eq!(
        doc["mem"]["booked_ram_gb"],
        json!(15.0),
        "two CPU residents book 15.0 GiB; the GPU row debits the card, never RAM: {}",
        doc["mem"]
    );
    assert_eq!(
        doc["mem"]["ram_reserve_gb"],
        json!(8.0),
        "the reserve the RAM gate subtracts rides beside the reading"
    );
    assert!(
        doc["mem"].get("ram_total_gb").is_some() && doc["mem"].get("ram_used_gb").is_some(),
        "the pre-existing host reading keys stay: {}",
        doc["mem"]
    );
    let clean = SchedulerStatus {
        slots: vec![occupied_slot("gpu-only", 9_302, now)],
        ..base.clone()
    };
    let doc = render_status_document(&api, &clean).await;
    assert_eq!(
        doc["mem"]["booked_ram_gb"],
        json!(0.0),
        "nothing loaded CPU is a 0.0 booking, not an absence: {}",
        doc["mem"]
    );
}

#[tokio::test]
async fn the_mem_block_never_presents_the_configured_total_as_measured() {
    let api = live_state(BootTimings::default());
    let status = api.scheduler.status().await.expect("actor alive");
    let doc = render_status_document(&api, &status).await;
    assert_eq!(
        doc["mem"]["total_basis"],
        json!("configured"),
        "a dead witness means the total is the declared budget: {}",
        doc["mem"]
    );
    assert_eq!(
        doc["mem"]["total_gb"],
        json!(api.config.vram_total_gb),
        "the numeric field stays for compatibility, but its basis says configured"
    );
    assert_eq!(
        doc["mem"]["configured_total_gb"],
        json!(api.config.vram_total_gb),
        "the configured budget rides beside the number in every arm"
    );
    assert_eq!(doc["mem"]["observed_used_gb"], json!(null));
    assert_eq!(doc["mem"]["source"], json!("unavailable"));
    assert_eq!(doc["mem"]["unavailable_code"], json!("witness_error"));
}
