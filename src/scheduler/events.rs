//! Actor-side event application: the event dispatch that advances a job through
//! its swap states, the one install funnel that seats an occupant, and the
//! serve-id re-discovery retry and fold-back.
//! The state, the job ledger, and the actor loop stay owned by `mod.rs`.

use crate::engine::{Probe, Spawned};
use crate::ownership::SpawnTicket;
use std::sync::Arc;
use std::time::Instant;

use super::measured::{serve_model_warning, SamplerCtx};
use super::{
    ActorMsg, ActorState, Event, JobId, JobKind, Occupant, Reply, SchedulerError,
    ServeIdProbeRelease, SwapReceipt, SwapState,
};

impl ActorState {
    pub(super) fn on_event(&mut self, job_id: JobId, event: Event) {
        self.apply_event(job_id, event);
        self.publish();
    }

    pub(super) fn apply_event(&mut self, job_id: JobId, event: Event) {
        let Some(job) = self.jobs.get(&job_id) else {
            return;
        };
        if job.terminal {
            return; // a stale step result after cancel/supersede
        }
        let state = job.state;
        // The admission decision is witness-driven; every other step
        // matches its expected state.
        if let Event::Admitted(card, ram) = event {
            if state == SwapState::Admit {
                self.finish_admit(job_id, card, ram);
            } else {
                tracing::warn!(%job_id, ?state, "stale admission sample ignored");
            }
            return;
        }
        match (state, event) {
            (SwapState::Dispose, Event::Released(Ok(elapsed))) => {
                self.release_samples.push(elapsed);
                if !self.set_timing(job_id, |t| &mut t.release_ms) {
                    return;
                }
                self.commit_victim_release(job_id);
                if let Some(victim) = self.jobs.get(&job_id).and_then(|job| job.victim.clone()) {
                    if let Some(job) = self.jobs.get_mut(&job_id) {
                        job.released_cost += victim.cost;
                        if victim.evicted {
                            job.victims_released.push(victim.model.clone());
                        } else {
                            // The same-slot holder's booked need is already inside
                            // `free_at_admit`. Crediting the same release there again
                            // is a double count, so it rides a separate accumulator.
                            job.holder_released_cost += victim.cost;
                        }
                    }
                }
                let job_kind = match self.jobs.get(&job_id).map(|job| job.kind) {
                    Some(kind) => kind,
                    None => {
                        self.log_invariant_missing_job(job_id, "release event");
                        return;
                    }
                };
                if job_kind == JobKind::Load {
                    // Release cross victims one at a time; after each
                    // release re-check the gate and stop as soon as it is satisfied
                    // (minimality — never kill more than the load needs).
                    if self.dequeue_victim(job_id) {
                        let Some((
                            target,
                            free_at_admit,
                            observed_at_admit,
                            released_cost,
                            holder_released_cost,
                        )) = self.jobs.get(&job_id).map(|job| {
                            let target = job
                                .gate_target
                                .unwrap_or_else(|| job.need.map_or(0.0, |n| n.0));
                            (
                                target,
                                job.free_at_admit.map_or(0.0, |f| f.0),
                                job.observed_at_admit,
                                job.released_cost,
                                job.holder_released_cost,
                            )
                        })
                        else {
                            self.log_invariant_missing_job(job_id, "victim release re-gate");
                            return;
                        };
                        let ledger_available =
                            free_at_admit + (released_cost - holder_released_cost);
                        let available = observed_at_admit.map_or(ledger_available, |obs| {
                            ledger_available.min(obs + released_cost)
                        });
                        if target <= available {
                            if let Some(job) = self.jobs.get_mut(&job_id) {
                                job.victims.clear();
                            }
                            tracing::info!(%job_id, "release sufficient; skipping remaining victims");
                            if !self.dequeue_reclaim_victim(job_id) {
                                self.spawn_settle_task(job_id);
                            } else {
                                self.spawn_kill_confirm(job_id);
                            }
                        } else {
                            self.spawn_kill_confirm(job_id);
                        }
                    } else if !self.dequeue_reclaim_victim(job_id) {
                        self.spawn_settle_task(job_id);
                    } else {
                        self.spawn_kill_confirm(job_id);
                    }
                } else {
                    let note = self
                        .jobs
                        .get(&job_id)
                        .and_then(|job| job.note.clone())
                        .unwrap_or_else(|| "unloaded".to_string());
                    self.step(job_id, SwapState::Reap);
                    self.step(job_id, SwapState::Idle);
                    self.finish(job_id, Ok(Reply::Done), &note);
                }
            }
            (SwapState::Dispose, Event::Released(Err(error))) => {
                let holder = self
                    .jobs
                    .get(&job_id)
                    .and_then(|job| job.victim.as_ref())
                    .map(|v| v.model.clone())
                    .unwrap_or_else(|| job_id.to_string());
                self.fail_job(
                    job_id,
                    SchedulerError::ReleaseTimeout { holder },
                    &error.to_string(),
                );
            }
            (SwapState::Spawn, Event::Spawned(Ok(spawned))) => {
                if let Some(ticket) = self.jobs.get_mut(&job_id).and_then(|j| j.spawned.as_mut()) {
                    ticket.port = spawned.port;
                    ticket.pid = spawned.pid;
                }
                if !self.set_timing(job_id, |t| &mut t.spawn_ms) {
                    return;
                }
                if !self.step(job_id, SwapState::Ready) {
                    return;
                }
                if !self.spawn_probe_task(job_id, spawned.port) {
                    self.fail_job(
                        job_id,
                        SchedulerError::InvariantViolation {
                            reason: "spawn readiness is set when a spawn exists".to_string(),
                        },
                        "spawn readiness missing",
                    );
                }
            }
            (SwapState::Spawn, Event::Spawned(Err(error))) => {
                // A failed client leaves the container it did start (or a
                // half-created one) reachable for exactly one more moment — snapshot
                // its log before this job's reap and the next launch's pre-reap erase
                // the engine's own account of the failure.
                self.snapshot_failed_container(job_id);
                self.fail_job(
                    job_id,
                    SchedulerError::Spawn(error.to_string()),
                    "spawn failed",
                );
            }
            (SwapState::Ready, Event::Probed(Probe::Ready, discovered)) => {
                self.install(job_id, discovered);
            }
            (SwapState::Ready, Event::Probed(Probe::Failed { reason }, _)) => {
                self.snapshot_failed_container(job_id);
                self.fail_job(
                    job_id,
                    SchedulerError::EngineFailed(reason),
                    "engine not ready",
                );
            }
            (SwapState::Ready, Event::Probed(Probe::NotYet { .. }, _)) => {
                tracing::warn!(%job_id, "probe task sent a non-terminal result; ignored");
            }
            (SwapState::Failed, Event::Released(release)) => {
                // A release that timed out here was never proven, and this
                // arm is the last moment anything in the scheduler still knows the
                // ticket — hand it to the one bounded late watch (option B). A confirmed
                // release needs no watch, and a job that never spawned owed none.
                // A confirmed release is a measured release whatever state reaped it:
                // it belongs in the wait-estimate ring like a Dispose one.
                match release {
                    Ok(elapsed) => self.release_samples.push(elapsed),
                    Err(_) => self.spawn_late_release_watch(job_id),
                }
                self.step(job_id, SwapState::Reap);
                self.step(job_id, SwapState::Idle);
                let error = self
                    .jobs
                    .get(&job_id)
                    .and_then(|job| job.error.clone())
                    .unwrap_or_else(|| SchedulerError::Spawn("unknown failure".to_string()));
                self.finish(job_id, Err(error), "failed");
            }
            (SwapState::CancelRequested, Event::Released(Ok(elapsed))) => {
                // The release this job's own (aborted) reap task never proved: now the
                // victim's engine is gone, so its slot must go the way a completed
                // Dispose release takes it, or the ledger debits a corpse forever
                // (and the victim's own row keeps claiming a Serving nobody can
                // serve). Confirmed means measured: the duration joins the
                // wait-estimate ring.
                self.release_samples.push(elapsed);
                self.commit_victim_release(job_id);
                self.finish_cancelled(job_id);
            }
            (SwapState::CancelRequested, Event::Released(Err(_))) => {
                // An unconfirmed release leaves the engine possibly alive: the slot
                // stays installed rather than claiming a death nobody proved.
                // This arm covers two shapes and only one of them needs the
                // watch. The UNINSTALLED spawn (the slot has no occupant, or holds a
                // different ticket) is the case this arm used to leave silent: after
                // `finish_cancelled` nothing in the scheduler can name the ticket again,
                // so hand it to the one bounded late watch (option B) now. The INSTALLED
                // one — a cancel that reached `Serving`, whose own ticket is the slot's
                // occupant — keeps G12's posture: the occupant stays installed, the next
                // restart or swap re-owns the ticket, and the watch stays out of it
                // ([`ActorState::late_watch_names_an_orphan`]).
                self.spawn_late_release_watch(job_id);
                self.finish_cancelled(job_id);
            }
            (SwapState::Dispose, Event::Settled(Ok(()))) => {
                if !self.begin_spawn(job_id) {
                    self.fail_job(
                        job_id,
                        Self::invariant_error("spawn follows a settled release"),
                        "spawn setup failed",
                    );
                }
            }
            (SwapState::Dispose, Event::Settled(Err(reason))) => {
                self.fail_job(
                    job_id,
                    SchedulerError::SettleTimeout { reason },
                    "memory did not settle after release",
                );
            }
            (other_state, other_event) => {
                tracing::warn!(state = ?other_state, event = ?other_event, %job_id, "unexpected event ignored");
            }
        }
    }

    /// `Ready -> Install -> Serving`: the one place an occupant is created.
    /// `discovered_id` is the engine's self-reported id.
    ///
    /// Retry contract: a `None` here is not the occupant's
    /// life sentence. The miss is a WARN (the model and what was tried), the
    /// load still succeeds, and the proxy keeps the documented harness-name
    /// fallback — but every later resolve that answers already-serving for
    /// this exact spawn ([`ActorState::on_swap`]) re-runs the same discovery
    /// off-thread; when the engine finally answers, [`ActorState::on_serve_id_refreshed`]
    /// lands the id on the occupant and the following forward carries it. The
    /// resolve itself never waits on or fails because of the probe.
    fn install(&mut self, job_id: JobId, discovered_id: Option<String>) {
        if !self.set_timing(job_id, |t| &mut t.ready_ms) {
            return;
        }
        if !self.step(job_id, SwapState::Install) {
            return;
        }
        let Some((slot, model, category, ticket)) = self.jobs.get(&job_id).map(|job| {
            (
                job.slot,
                job.model.clone(),
                job.category,
                job.spawned.clone(),
            )
        }) else {
            self.log_invariant_missing_job(job_id, "install");
            return;
        };
        let Some(ticket) = ticket else {
            tracing::error!(%job_id, "scheduler invariant violation: install only happens after a spawn");
            self.finish(
                job_id,
                Err(SchedulerError::InvariantViolation {
                    reason: "install only happens after a spawn".to_string(),
                }),
                "invariant",
            );
            return;
        };
        let Some(slot) = slot else {
            tracing::error!(%job_id, "scheduler invariant violation: install needs a slot");
            self.finish(
                job_id,
                Err(SchedulerError::InvariantViolation {
                    reason: "install needs a slot".to_string(),
                }),
                "invariant",
            );
            return;
        };
        let port = Spawned {
            port: ticket.port,
            pid: 0,
        };
        let serve_id = discovered_id;
        if serve_id.is_none() {
            tracing::warn!(
                %job_id,
                model = %model,
                port = port.port,
                "engine reported no model id: tried GET /v1/models at readiness; \
                 the proxy forwards the harness name and re-discovers on every \
                 resolve until an id lands"
            );
        }
        if let Some(job) = self.jobs.get_mut(&job_id) {
            if let Some(warning) = serve_model_warning(job.measured.as_ref(), serve_id.as_deref()) {
                tracing::warn!(%job_id, model = %job.model, "{warning}");
                job.warnings.push(warning);
            }
        }
        if let Some(job) = self.jobs.get(&job_id).map(|job| {
            (
                job.model.clone(),
                job.need.unwrap_or_default(),
                job.ram_need,
            )
        }) {
            let (job_model, job_need, job_ram_need) = job;
            self.clear_occupant(slot);
            let others_booked = self
                .slots
                .iter()
                .filter_map(|s| s.occupant.as_ref())
                .filter(|o| o.model != job_model)
                .map(|o| o.need.0)
                .sum();
            if let Some(job) = self.jobs.get_mut(&job_id) {
                job.other_need_at_admit = others_booked;
            }
            let spawn_config = self
                .jobs
                .get(&job_id)
                .and_then(|job| job.measured.as_ref())
                .and_then(|anchor| {
                    anchor.file.as_ref().and_then(|file| {
                        crate::model::fingerprint(file).map(|fp| (file.clone(), fp))
                    })
                });
            if let Some(slot_state) = self.slots.get_mut(slot) {
                slot_state.occupant = Some(Occupant {
                    model: job_model,
                    need: job_need,
                    ram_need: job_ram_need,
                    port: port.port,
                    ticket: ticket.clone(),
                    loaded_at: Instant::now(),
                    launched_at_wall: std::time::SystemTime::now(),
                    spawn_config,
                    stale_config: false,
                    serve_id: serve_id.clone(),
                    measure: None,
                    live_measured_gb: None,
                });
            } else {
                self.finish(
                    job_id,
                    Err(SchedulerError::InvariantViolation {
                        reason: "install slot exists".to_string(),
                    }),
                    "invariant",
                );
                return;
            }
        } else {
            self.log_invariant_missing_job(job_id, "install occupant");
            return;
        }
        // Measured footprint (§0.4, rehearsal item c): witness readings taken while the
        // model serves, folded into its yaml range. The baseline is the free memory
        // *after* this load's own release, so a replacement is not measured short by the
        // victim's whole footprint. The card's per-process attribution is preferred
        // when the driver reports it; the total-diff stands in otherwise, which is what
        // makes a container-backed engine measurable at all (its ticket pid is a
        // short-lived `wsl.exe` client) — and the container's own startup
        // `MemorySummary` stands in ahead of even that, so the sampler fetches it
        // first. What to charge is the actor's call alone: it knows who else is booked
        // and which engine actually holds the slot.
        let sampler_ctx = self.jobs.get(&job_id).map(|job| SamplerCtx {
            model: job.model.clone(),
            own_pids: if job.container.is_none() {
                vec![ticket.pid]
            } else {
                Vec::new()
            },
            ticket: ticket.clone(),
            container: job.container.clone(),
            engine: job.engine.clone(),
        });
        // A CPU-resident resident walks the RAM ladder's own sampler instead
        // of the card witness one:
        // the card sampler would fold host-RAM growth into the yaml's
        // `measured_gb` — a VRAM figure — so the install branch ends before
        // it ever starts, and the working-set loop takes the handle. Its
        // `booked` starts at the admit-time booking: a floor, not a reading,
        // so the loop's first sample arrives a full cadence after install.
        let ram_seed = self
            .jobs
            .get(&job_id)
            .and_then(|job| job.ram_need)
            .map(|need| need.0);
        let baseline_free = self.post_release_free(job_id);
        if let Some(ctx) = sampler_ctx {
            let witness = Arc::clone(&self.witness);
            let runtime = Arc::clone(&self.runtime);
            let tx = self.tx.clone();
            let probe = Arc::clone(&self.working_set_probe);
            let every = std::time::Duration::from_secs(self.cfg.measured_sample_s.max(1));
            let handle = tokio::spawn(async move {
                if let Some(booked) = ram_seed {
                    Self::ram_refresh(&runtime, &tx, &ctx, &probe, booked, every).await;
                    return;
                }
                // The container's own startup report is demanded whenever a container
                // is installed — it is a property of the engine, not of the witness,
                // and a card the witness cannot observe (ledger-only, wedged driver)
                // must not also blind the harness to what the engine itself said.
                if Self::report_container(&runtime, &tx, &ctx).await.is_some() {
                    return;
                }
                // The process engines basalt/strata land their own status
                // figure (or the process-tree witness sum it falls back to)
                // right here at readiness. These rows are ELASTIC
                // engines — the expert caches keep moving after the first
                // fold — so instead of ending the sampler the way ninfer's
                // `engine_report` ends its one-shot (ninfer semantics stay
                // untouched: that path never reaches here), the landed figure
                // seeds the elastic refresh loop, which re-folds the engine's
                // own status word under the asymmetric hysteresis band
                // ([`ActorState::elastic_refresh`]: up freely, down only past
                // the band). Anything the landing refuses (silence, an
                // unmeasurable figure) returns None and falls through to the
                // witness loop below, so the stand-in readings keep flowing
                // and a refused landing never consumes them (review
                // 2026-10-31).
                if matches!(ctx.engine.as_str(), "basalt" | "strata" | "xolotl") {
                    if let Some(booked) =
                        Self::report_measurement(&runtime, &tx, &ctx, &witness).await
                    {
                        Self::elastic_refresh(&runtime, &tx, &ctx, &witness, booked, every).await;
                        return;
                    }
                }
                // The total-diff stand-in needs the admit reading as its baseline;
                // with no observation to subtract from, there is no honest
                // difference to take, so the stand-in — and only the stand-in —
                // stays home.
                let Some(baseline_free) = baseline_free else {
                    return;
                };
                let mut tick = tokio::time::interval(every);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // The first interval tick completes at once; consume it, because a
                // sample taken the instant the port opened measures the state the load
                // was admitted in — and it would spend a scripted witness sample.
                tick.tick().await;
                // Bounded by wall clock, not by tick count: a 86_400-iteration loop at
                // the 60 s default would run for sixty days.
                let deadline = tokio::time::Instant::now() + super::SAMPLER_LIFETIME_BOUND;
                loop {
                    tick.tick().await;
                    if tokio::time::Instant::now() >= deadline {
                        return;
                    }
                    match witness.sample().await {
                        Ok(sample) => {
                            // `free_gib()` clamps, so a NaN or nonsense reading would
                            // otherwise arrive as "the model took the whole card" and
                            // be persisted to the operator's file forever.
                            if !sample.total_gib.is_finite()
                                || !sample.used_gib.is_finite()
                                || sample.total_gib <= 0.0
                                || sample.used_gib < 0.0
                            {
                                continue;
                            }
                            let total_diff = baseline_free - (sample.total_gib - sample.used_gib);
                            if !total_diff.is_finite() {
                                continue;
                            }
                            if tx
                                .send(ActorMsg::Measured {
                                    model: ctx.model.clone(),
                                    ticket: ctx.ticket.clone(),
                                    total_diff,
                                    own: sample.own_gib(&ctx.own_pids),
                                    total_gib: sample.total_gib,
                                    used_gib: sample.used_gib,
                                })
                                .is_err()
                            {
                                return; // actor gone
                            }
                        }
                        // A witness that cannot answer yields no measurement, never a
                        // false one (§0.3): stop rather than guess or hammer it.
                        Err(_) => return,
                    }
                }
            });
            if let Some(occupant) = self.slots.get_mut(slot).and_then(|s| s.occupant.as_mut()) {
                occupant.measure = Some(handle);
            }
        }
        if !self.set_timing(job_id, |t| &mut t.install_ms) {
            return;
        }
        if !self.step(job_id, SwapState::Serving) {
            return;
        }
        self.finish(
            job_id,
            Ok(Reply::Loaded(SwapReceipt {
                job: job_id,
                model,
                category,
                port: port.port,
                serve_id,
            })),
            "loaded",
        );
    }

    /// Retries serve-id discovery for an occupant that installed with none
    /// (contract on [`ActorState::install`]): one
    /// [`crate::engine::EngineRuntime::discover_model_id`] per spawn at a time, off the
    /// actor, answered back as [`ActorMsg::ServeIdRefreshed`] whatever the
    /// probe's exit — so a resolve never waits on or fails because of the
    /// probe, and a silent engine is retried by the next resolve rather than
    /// latched.
    pub(super) fn retry_serve_id(&mut self, ticket: SpawnTicket) {
        if !self
            .serve_id_refresh
            .insert((ticket.port, ticket.generation))
        {
            return;
        }
        let tx = self.tx.clone();
        let runtime = Arc::clone(&self.runtime);
        let (port, generation) = (ticket.port, ticket.generation);
        tokio::spawn(async move {
            // RAII release (the release-per-exit rule): the in-flight mark clears on EVERY
            // exit — a panic in the probe reports as silence, never as a mark
            // wedged for the process lifetime.
            let mut probe = ServeIdProbeRelease {
                tx,
                port,
                generation,
                outcome: None,
            };
            probe.outcome = Some(runtime.discover_model_id(port).await);
        });
    }

    /// Folds a re-discovery answer back onto its occupant.
    ///
    /// # Arguments
    ///
    /// * `port` / `generation` — the spawn the probe was launched for; an
    ///   answer for a replaced occupant is dropped, never laundered onto a new
    ///   engine that was installed with a real id.
    /// * `discovered` — the id the engine now reports, `None` for continued
    ///   silence (named ONCE per unchanged silence state per spawn, the
    ///   reconcile doctrine; the next resolve retries).
    ///
    /// # Returns
    ///
    /// `true` only when an id actually landed on a still-serving, id-less
    /// occupant — the one change worth publishing.
    pub(super) fn on_serve_id_refreshed(
        &mut self,
        port: u16,
        generation: u64,
        discovered: Option<&str>,
    ) -> bool {
        let Some(slot) = self.slots.iter().position(|s| {
            s.occupant
                .as_ref()
                .is_some_and(|o| o.ticket.port == port && o.ticket.generation == generation)
        }) else {
            return false;
        };
        let Some(occupant) = self
            .slots
            .get_mut(slot)
            .and_then(|slot| slot.occupant.as_mut())
        else {
            return false;
        };
        let Some(id) = discovered else {
            // Warn on the transition INTO silence, never on every landed
            // silence: a client hammering a permanently
            // silent engine used to log ~0.5 warns/s/slot forever.
            if self.serve_id_silence_warned.insert((port, generation)) {
                tracing::warn!(
                    model = %occupant.model,
                    port,
                    "serve_id re-discovery still reports nothing; the proxy forwards \
                     the harness name and the next resolve retries"
                );
            }
            return false;
        };
        if occupant.serve_id.is_some() {
            return false;
        }
        // The landed id ends the silence state: a later silence for this same
        // spawn is a new outcome and may name itself once again.
        self.serve_id_silence_warned.remove(&(port, generation));
        tracing::info!(
            model = %occupant.model,
            port,
            serve_id = %id,
            "serve_id re-discovery landed; the next forward carries the engine id"
        );
        occupant.serve_id = Some(id.to_string());
        true
    }
}
