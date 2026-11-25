use super::*;

// --- launch validation precedes eviction --------------------------------------------

/// Pins the refusal of a load the adapter cannot build a spec for: terminal at the
/// admission stage, naming what it refused, and leaving the card exactly as it was —
/// the resident model still serving and nothing evicted.
///
/// # Arguments
///
/// * `status` - the snapshot taken after the refusal.
/// * `bad_model` - the model whose yaml the adapter refuses.
/// * `resident_category` / `resident_model` - the occupant that was still on the card
///   (the same-slot replacement or the planned cross-category victim).
/// * `names` - the fragment of the error that must name the refused key or value.
fn assert_refused_without_damage(
    status: &simpleharn::scheduler::SchedulerStatus,
    bad_model: &str,
    resident_category: CategoryId,
    resident_model: &str,
    names: &str,
) {
    let job = status
        .jobs
        .iter()
        .find(|j| j.model == bad_model)
        .expect("the refused load is in the job ledger");
    assert!(job.terminal, "the refusal must be terminal: {job:?}");
    assert_eq!(
        slot_model(status, resident_category),
        vec![Some(resident_model.to_string())],
        "the resident model must still hold its slot"
    );
    assert!(
        job.victims_released.is_empty(),
        "a refused launch may evict nobody: {:?}",
        job.victims_released
    );
    assert_eq!(
        job.state,
        SwapState::AdmitFail,
        "a launch that cannot be built is refused before Quiesce/Dispose: {job:?}"
    );
    assert_eq!(job.outcome.as_deref(), Some("launch refused"), "{job:?}");
    let error = job.error.as_deref().unwrap_or_default();
    assert!(
        error.contains(names),
        "the error must name `{names}`, was: {error}"
    );
}

#[tokio::test]
async fn a_strata_model_without_a_profile_source_is_refused_before_the_victim_dies() {
    // DECISIONS row 19 shape: the template's `strata_exe` is required by the
    // real adapter and existence checked there. 30 (AUX) + 60 (WORK) > 72 - 2, so
    // admission plans aux30 as the victim: without the pre-Dispose preflight the
    // kill runs first and the refusal arrives over a corpse.
    let (handle, runtime) = harness5(
        config(5),
        &[
            ("aux30", "mock", CategoryId::Aux, Some(30.0), ""),
            ("strata-nocfg", "strata", CategoryId::Work, Some(60.0), ""),
        ],
    );
    let Reply::Loaded(resident) = handle
        .swap("aux30", CategoryId::Aux)
        .await
        .expect("the resident model loads")
    else {
        panic!("expected Loaded");
    };
    assert_eq!(runtime.server_count(), 1, "the resident engine is live");

    let error = handle
        .swap("strata-nocfg", CategoryId::Work)
        .await
        .expect_err("a missing profile source must refuse the load");
    assert!(
        error.to_string().contains("required field `strata_exe`"),
        "the refusal must name the missing key, was: {error}"
    );

    let status = handle.status().await.expect("actor alive");
    assert_refused_without_damage(
        &status,
        "strata-nocfg",
        CategoryId::Aux,
        "aux30",
        "required field `strata_exe`",
    );
    assert_eq!(runtime.server_count(), 1, "nothing killed, nothing spawned");
    assert!(
        !runtime.is_dead(resident.port).await,
        "the planned victim must still be answering on its engine port"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn an_illegal_ninfer_container_name_is_refused_before_the_occupant_dies() {
    // The security refusal of the preflight: a `ninfer_container` docker would not
    // accept is rejected by the real adapter — and must be rejected while the model
    // this load replaces is still serving.
    let (handle, runtime) = harness5(
        config(5),
        &[
            ("work-a", "mock", CategoryId::Work, Some(4.0), ""),
            (
                "ninfer-badname",
                "ninfer",
                CategoryId::Work,
                Some(8.0),
                "ninfer_image: ninfer:local\nninfer_container: \"bad container\"",
            ),
        ],
    );
    let Reply::Loaded(resident) = handle
        .swap("work-a", CategoryId::Work)
        .await
        .expect("the resident model loads")
    else {
        panic!("expected Loaded");
    };

    let error = handle
        .swap("ninfer-badname", CategoryId::Work)
        .await
        .expect_err("an illegal container name must refuse the load");
    assert!(
        error
            .to_string()
            .contains("ninfer_container must be a docker name"),
        "the refusal must name the illegal value, was: {error}"
    );

    let status = handle.status().await.expect("actor alive");
    assert_refused_without_damage(
        &status,
        "ninfer-badname",
        CategoryId::Work,
        "work-a",
        "ninfer_container must be a docker name",
    );
    assert_eq!(runtime.server_count(), 1, "nothing killed, nothing spawned");
    assert!(
        !runtime.is_dead(resident.port).await,
        "the replaced model must still be answering on its engine port"
    );
}

#[tokio::test]
async fn a_llama_model_without_its_model_key_is_refused_before_the_occupant_dies() {
    // `model:` is llama's one required key (Appendix A.7); scan accepts the file, so
    // the adapter is the first place that can see it — and the first place it may be
    // seen, before any eviction may run.
    let (handle, runtime) = harness5(
        config(5),
        &[
            ("aux-a", "mock", CategoryId::Aux, Some(4.0), ""),
            (
                "llama-no-model",
                "llama",
                CategoryId::Aux,
                Some(8.0),
                "context: 4096",
            ),
        ],
    );
    let Reply::Loaded(resident) = handle
        .swap("aux-a", CategoryId::Aux)
        .await
        .expect("the resident model loads")
    else {
        panic!("expected Loaded");
    };

    let error = handle
        .swap("llama-no-model", CategoryId::Aux)
        .await
        .expect_err("a llama entry with no model path must refuse the load");
    assert!(
        error.to_string().contains("required field `model`"),
        "the refusal must name the missing key, was: {error}"
    );

    let status = handle.status().await.expect("actor alive");
    assert_refused_without_damage(
        &status,
        "llama-no-model",
        CategoryId::Aux,
        "aux-a",
        "required field `model`",
    );
    assert_eq!(runtime.server_count(), 1, "nothing killed, nothing spawned");
    assert!(
        !runtime.is_dead(resident.port).await,
        "the replaced model must still be answering on its engine port"
    );
}
