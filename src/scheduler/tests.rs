use super::{
    gather_drift_stats, headline_fault, measured, missing_binary_error, serve_model_warning,
    watch_actor, ActorMsg, ActorState, AdmissionError, AdmissionPath, DriftStats, Event, Gibibytes,
    Job, JobId, JobKind, JobStatus, JobTimings, MeasuredAnchor, MeasuredData, Occupant, Reading,
    ReleaseSamples, Reply, ResourceKind, ScanKind, Scheduler, SchedulerError, SchedulerHealth,
    Slot, SpawnTicket, SwapState, Victim, RELEASE_POLL, RELEASE_SAMPLE_CAP,
};
use crate::budget::LedgerOnlyWitness;
use crate::config::CategoryId;
use crate::engine::mock::MockAdapter;
use crate::engine::EngineAdapter;
use crate::model::ModelEntry;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod booking;
mod catalog;
mod death;
mod drift;
mod expert;
mod fault;
mod fit;
mod gate;
mod park;
mod preflight;
mod ram;
mod release;
mod swap;
mod witness;
mod workset;

/// Process-global warn capture for the persist-drift test: `tracing`'s default
/// dispatcher is thread-local, and the persist warn fires on a blocking-pool
/// thread, so the capturing layer goes in as the global default exactly once
/// (no lib test installs one; `observability::init` runs only in `main`).
type CapturedWarn = (String, BTreeMap<String, String>);
static WARN_CAPTURE: OnceLock<Arc<Mutex<Vec<CapturedWarn>>>> = OnceLock::new();

struct CaptureVisitor<'a> {
    message: &'a mut String,
    fields: &'a mut BTreeMap<String, String>,
}

impl tracing::field::Visit for CaptureVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.message = format!("{value:?}");
        } else {
            self.fields
                .insert(field.name().to_string(), format!("{value:?}"));
        }
    }
}

struct CaptureLayer(Arc<Mutex<Vec<CapturedWarn>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CaptureLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut message = String::new();
        let mut fields = BTreeMap::new();
        event.record(&mut CaptureVisitor {
            message: &mut message,
            fields: &mut fields,
        });
        self.0.lock().expect("capture lock").push((message, fields));
    }
}

fn install_warn_capture() -> Arc<Mutex<Vec<CapturedWarn>>> {
    WARN_CAPTURE
        .get_or_init(|| {
            let store = Arc::new(Mutex::new(Vec::new()));
            let _ = tracing_subscriber::registry()
                .with(CaptureLayer(store.clone()))
                .try_init();
            store
        })
        .clone()
}

/// Constructor-only stand-in for the never-called runtime half of an
/// [`ActorState`]: the boot-thread pin builds state and reads its catalog field,
/// it never spawns, kills or probes, so any call reaching here is a test bug.
struct NeverRuntime;

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for NeverRuntime {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        _ticket: crate::ownership::SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        panic!("the boot-thread pin must never call the runtime");
    }
    async fn kill(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<(), crate::engine::SpawnError> {
        panic!("the boot-thread pin must never call the runtime");
    }
    async fn release_confirmed(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        panic!("the boot-thread pin must never call the runtime");
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        panic!("the boot-thread pin must never call the runtime");
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        panic!("the boot-thread pin must never call the runtime");
    }
}

struct BootWiring {
    adapters: BTreeMap<String, Arc<dyn EngineAdapter>>,
    runtime: Arc<dyn crate::engine::EngineRuntime>,
    witness: Arc<dyn crate::budget::VramWitness>,
    tx: tokio::sync::mpsc::UnboundedSender<super::ActorMsg>,
}

fn boot_stub_wiring() -> BootWiring {
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    BootWiring {
        adapters,
        runtime: Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        witness: Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx: tokio::sync::mpsc::unbounded_channel().0,
    }
}

/// Seats a spawn exactly as the actor does: the terminal load row that owns the
/// ticket — carrying the spawn-era [`MeasuredAnchor`] built by the very function
/// [`ActorState::begin_spawn`] calls — plus the occupant that holds its sampler.
/// A pin that folds a reading must go through this: with no spawn row there is no
/// anchor, and an unanchored reading has neither an honest stamp to mint nor a
/// file it may write to.
fn seat_spawn(state: &mut ActorState, entry: &ModelEntry, ticket: &SpawnTicket, port: u16) {
    let job_id = JobId(state.next_job);
    state.next_job += 1;
    state.jobs.insert(
        job_id,
        Job {
            kind: JobKind::Load,
            // Seated directly, not admitted: the row never took a rung.
            path: None,
            model: entry.name.clone(),
            engine: entry.engine.clone(),
            category: CategoryId::Work,
            state: SwapState::Serving,
            terminal: true,
            outcome: Some("loaded".to_string()),
            error: None,
            need: Some(Gibibytes(2.0)),
            resource: entry.resource(),
            ram_need: None,
            cost: entry.cost,
            vram_gate: entry.vram_gate,
            load_timeout_s: entry.load_timeout_s,
            gate_target: Some(2.0),
            free_at_admit: Some(Gibibytes(20.0)),
            observed_at_admit: Some(20.0),
            release_first: false,
            container: None,
            timings: JobTimings::default(),
            t0: Instant::now(),
            waiters: Vec::new(),
            tasks: Vec::new(),
            spawned: Some(ticket.clone()),
            readiness: None,
            measured: Some(MeasuredAnchor::at_spawn(entry)),
            victim: None,
            victims: Vec::new(),
            reclaim: super::Reclaim::default(),
            victims_released: Vec::new(),
            released_cost: 0.0,
            holder_released_cost: 0.0,
            other_need_at_admit: 0.0,
            note: Some("loaded".to_string()),
            warnings: Vec::new(),
            slot: Some(0),
            queue_fingerprint: None,
        },
    );
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: port,
        occupant: Some(Occupant {
            model: entry.name.clone(),
            need: Gibibytes(2.0),
            ram_need: None,
            port,
            ticket: ticket.clone(),
            loaded_at: Instant::now(),
            launched_at_wall: std::time::SystemTime::now(),
            spawn_config: None,
            stale_config: false,
            serve_id: None,
            measure: None,
            live_measured_gb: None,
        }),
        job: Some(job_id),
    }];
}

impl ActorState {
    /// The pin seam for one witness reading, exactly as the serving-window sampler
    /// sends it (`ActorMsg::Measured`).
    fn witness(&mut self, model: &str, ticket: &SpawnTicket, attributed: f64) {
        self.on_measured(
            model,
            ticket,
            super::Reading {
                total_diff: attributed,
                own: Some(attributed),
                total_gib: 24.0,
                used_gib: attributed + 0.1,
            },
        );
    }

    /// The pin seam for the container's one-shot startup `MemorySummary` figure
    /// (`ActorMsg::MeasuredEngineReport`). The card is the operator's 72 GiB: a
    /// reserved figure may legitimately exceed the 24 GiB the witness fold pins
    /// use, because it counts KV reserved at *declared* capacity, not bytes the
    /// card was observed carrying.
    fn engine_report(&mut self, model: &str, ticket: &SpawnTicket, required_gb: f64) {
        self.on_engine_report(model, ticket, required_gb, 72.0);
    }

    /// The pin seam for the process engines' one-shot measurement
    /// (`ActorMsg::MeasuredProcessReport`), on the same 72 GiB card the
    /// `engine_report` seam assumes: a status figure counts what the
    /// engine holds, not only bytes the witness was sampling at one tick.
    fn process_measurement(
        &mut self,
        model: &str,
        ticket: &SpawnTicket,
        measurement: crate::engine::EngineMeasurement,
    ) {
        self.on_process_measurement(model, ticket, measurement, 72.0);
    }

    /// The pin seam for the persist-failed reconcile: the disk refused exactly this
    /// figure, so the drain must land on exactly these values. Pair-only rows pass
    /// `attribution: None, profile: None` — the legacy shape a pre-provenance
    /// persist carried.
    fn fail_persist(
        &mut self,
        model: &str,
        data: super::MeasuredData,
        file: Option<std::path::PathBuf>,
    ) {
        self.on_measured_persist_failed(model, &data, file);
    }
}

/// Poll a persisted yaml until every needle appears. The measured persist
/// runs on the blocking pool and lands through one atomic temp+rename, so a
/// successful write publishes the whole batch in one step.
async fn await_yaml_keys(path: &std::path::Path, keys: &[&str]) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(path).expect("persisted yaml readable");
        if keys.iter().all(|k| text.contains(k)) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "keys {keys:?} never reached the disk: {text}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Actor state for the fold pins: the fold consults no adapter, runtime or
/// witness (each pin drives [`ActorState::on_measured`] with its own reading), so
/// the never-called stand-ins keep the setup to one line per test.
fn fold_state(dir: &std::path::Path) -> ActorState {
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    ActorState::empty(
        crate::config::AppConfig::default(),
        dir.to_path_buf(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    )
}

/// The ticket a fold pin seats its spawn under. Any generation works: the fold's
/// `(model, ticket)` guard is satisfied by `seat_spawn` writing the same value.
fn fold_ticket(port: u16) -> SpawnTicket {
    SpawnTicket {
        generation: 7,
        kind: ResourceKind::ChildProcess,
        pid: 4242,
        port,
    }
}

/// Reads one yaml of the temp dir back through the real parser, as `reload()`
/// would: `read_measured` runs here, so the flip-void rule fires exactly as it
/// does on a rescan — the pins cannot fake the catalog into any shape the scan
/// would not have produced.
fn rescan_entry(path: &std::path::Path, name: &str) -> ModelEntry {
    let text = std::fs::read_to_string(path).expect("fixture yaml readable");
    let (map, problems) = crate::model::scan::parse_flat_yaml(&text);
    assert!(problems.is_empty(), "{problems:?}");
    let mut entry = ModelEntry::from_flat(&map, name).expect("entry");
    entry.file = Some(path.to_path_buf());
    entry
}

/// Scripted witness for the sampler pins: one canned sample answers every
/// `sample()`. No engine, no driver — the SE fake-everything rule.
struct ScriptedWitness(crate::budget::MemorySample);

#[async_trait::async_trait]
impl crate::budget::VramWitness for ScriptedWitness {
    async fn sample(&self) -> Result<crate::budget::MemorySample, String> {
        Ok(self.0.clone())
    }
    fn source(&self) -> String {
        "scripted".to_string()
    }
}

/// Scripted runtime for the sampler pins: a canned status-snapshot answer
/// (or silence) and a canned direct-children list. Everything else keeps
/// the NeverRuntime posture — a load reaching this runtime is a test bug.
struct StatusRuntime {
    status: Option<serde_json::Value>,
    children: Vec<u32>,
}

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for StatusRuntime {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        _ticket: crate::ownership::SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        panic!("the measurement sampler pin must never load");
    }
    async fn kill(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<(), crate::engine::SpawnError> {
        panic!("the measurement sampler pin must never load");
    }
    async fn release_confirmed(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        panic!("the measurement sampler pin must never load");
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        panic!("the measurement sampler pin must never load");
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        panic!("the measurement sampler pin must never load");
    }
    async fn engine_status_json(&self, _port: u16, _path: &str) -> Option<serde_json::Value> {
        self.status.clone()
    }
    async fn descendant_pids(&self, _pid: u32) -> Vec<u32> {
        self.children.clone()
    }
}

fn sampler_ctx(model: &str, engine: &str, ticket: SpawnTicket) -> super::measured::SamplerCtx {
    super::measured::SamplerCtx {
        model: model.to_string(),
        ticket,
        own_pids: Vec::new(),
        container: None,
        engine: engine.to_string(),
    }
}

/// The witness shape the sampler pins share: a healthy 72 GiB card, one
/// scripted per-process table.
fn scripted_sample(processes: Vec<(u32, f64)>) -> ScriptedWitness {
    let attribution = crate::budget::attribution_from_processes(&processes);
    ScriptedWitness(crate::budget::MemorySample {
        total_gib: 72.0,
        used_gib: 40.0,
        processes,
        attribution,
    })
}

/// Shared fixture for the Axis-2 resident pins: one `ActorState` with one
/// resident whose `spawn_config` is a real capture of a real yaml (the state
/// `install` produces — same fields, taken from the same helper), the file's
/// fingerprint, and the entry pair the fold would have written.
fn resident_on_a_file() -> (
    ActorState,
    std::path::PathBuf,
    crate::model::FileFingerprint,
) {
    let dir = crate::testutil::temp_dir("stale-resident");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let yaml = dir.join("m.yaml");
    std::fs::write(
        &yaml,
        "engine: mock\ncost: fixed\nmeasured_gb: 10.0\nmeasured_gb_min: 8.0\n",
    )
    .expect("seed");
    let (tx, _rx) = mpsc::unbounded_channel::<ActorMsg>();
    let mut adapters: BTreeMap<String, Arc<dyn EngineAdapter>> = BTreeMap::new();
    adapters.insert(
        "mock".to_string(),
        Arc::new(MockAdapter) as Arc<dyn EngineAdapter>,
    );
    let mut state = ActorState::empty(
        crate::config::AppConfig::default(),
        dir.clone(),
        adapters,
        Arc::new(NeverRuntime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    let capture = crate::model::fingerprint(&yaml).expect("seeded file fingerprints");
    let ticket = SpawnTicket {
        generation: 1,
        kind: ResourceKind::ChildProcess,
        pid: 4242,
        port: 18_431,
    };
    state.catalog = vec![entry_from(&yaml, "m")];
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: 18_431,
        occupant: Some(Occupant {
            model: "m".to_string(),
            need: Gibibytes(4.0),
            ram_need: None,
            port: 18_431,
            ticket,
            loaded_at: Instant::now(),
            launched_at_wall: std::time::SystemTime::now(),
            spawn_config: Some((yaml.clone(), capture)),
            stale_config: false,
            serve_id: None,
            measure: None,
            live_measured_gb: None,
        }),
        job: None,
    }];
    (state, yaml, capture)
}

fn entry_from(path: &std::path::Path, name: &str) -> ModelEntry {
    let text = std::fs::read_to_string(path).expect("readable");
    let (map, _) = crate::model::scan::parse_flat_yaml(&text);
    let mut entry = ModelEntry::from_flat(&map, name).expect("entry");
    entry.file = Some(path.to_path_buf());
    entry
}

/// Mirrors the off-actor gather a real reload performs: the resident capture
/// files plus the queued models' new-catalog entry files, stat'ed here on the
/// test thread (production stats run on a blocking thread).
fn drift_stats_for(state: &ActorState, catalog: &crate::model::scan::Catalog) -> DriftStats {
    let queued_at_dispatch: HashSet<JobId> = state.queued_load_candidates().into_iter().collect();
    let queued = state.queued_models_for(&queued_at_dispatch);
    let mut paths = state.drift_capture_files();
    paths.extend(
        catalog
            .models
            .iter()
            .filter(|entry| queued.contains(&entry.name))
            .filter_map(|entry| entry.file.clone()),
    );
    DriftStats {
        queued_at_dispatch,
        ..gather_drift_stats(&paths)
    }
}

/// The reload-turn rescan the pins drive: re-parse the file the operator (or the
/// harness) just wrote and land it through `apply_catalog` — the only judge
/// between install and the next launch.
fn rescan(state: &mut ActorState, yaml: &std::path::Path, name: &str) {
    let catalog = crate::model::scan::Catalog {
        models: vec![entry_from(yaml, name)],
        errors: Vec::new(),
    };
    let drift = drift_stats_for(state, &catalog);
    state.apply_catalog(catalog, None, &drift);
}

/// Seats a load row that owns a spawn, in the state and slot shape the pin is about
/// to drive. The ticket is live in the runtime either way; `occupant` is what the
/// slot reports.
fn seat_spawn_row(
    state: &mut ActorState,
    name: &str,
    job_state: SwapState,
    ticket: &SpawnTicket,
    occupant: Option<Occupant>,
) -> JobId {
    let job_id = JobId(state.next_job);
    state.next_job += 1;
    state.jobs.insert(
        job_id,
        Job {
            kind: JobKind::Load,
            // Seated directly, not admitted: the row never took a rung.
            path: None,
            model: name.to_string(),
            engine: "mock".to_string(),
            category: CategoryId::Work,
            state: job_state,
            terminal: false,
            outcome: None,
            error: None,
            need: Some(Gibibytes(2.0)),
            resource: crate::model::Resource::Gpu,
            ram_need: None,
            cost: crate::model::CostClass::Fixed,
            vram_gate: crate::model::VramGate::default(),
            load_timeout_s: None,
            gate_target: None,
            free_at_admit: None,
            observed_at_admit: None,
            release_first: false,
            container: None,
            timings: JobTimings::default(),
            t0: Instant::now(),
            waiters: Vec::new(),
            tasks: Vec::new(),
            spawned: Some(ticket.clone()),
            readiness: None,
            measured: None,
            victim: None,
            victims: Vec::new(),
            reclaim: super::Reclaim::default(),
            victims_released: Vec::new(),
            released_cost: 0.0,
            holder_released_cost: 0.0,
            other_need_at_admit: 0.0,
            note: None,
            warnings: Vec::new(),
            slot: Some(0),
            queue_fingerprint: None,
        },
    );
    state.slots = vec![Slot {
        category: CategoryId::Work,
        index: 0,
        engine_port: ticket.port,
        occupant,
        job: Some(job_id),
    }];
    job_id
}

/// An installed occupant as [`ActorState::install`] seats one: the ticket it was
/// spawned with, and nothing else the pins need.
fn seated_occupant(name: &str, ticket: &SpawnTicket) -> Occupant {
    Occupant {
        model: name.to_string(),
        need: Gibibytes(2.0),
        ram_need: None,
        port: ticket.port,
        ticket: ticket.clone(),
        loaded_at: Instant::now(),
        launched_at_wall: std::time::SystemTime::now(),
        spawn_config: None,
        stale_config: false,
        serve_id: None,
        measure: None,
        live_measured_gb: None,
    }
}

/// The installed-occupant shape: a `Serving` row whose OWN spawn is the slot's installed
/// occupant, held non-terminal so a cancel can reach it — the cancel-of-a-serving-
/// model case where G12 keeps the occupant installed if the reap cannot prove the
/// death, and `/har/status` goes on reporting the model as serving.
fn serving_occupant_row(state: &mut ActorState, name: &str, ticket: &SpawnTicket) -> JobId {
    seat_spawn_row(
        state,
        name,
        SwapState::Serving,
        ticket,
        Some(seated_occupant(name, ticket)),
    )
}

fn gate_sample(used_gib: f64) -> crate::budget::MemorySample {
    crate::budget::MemorySample {
        total_gib: 72.0,
        used_gib,
        processes: Vec::new(),
        attribution: crate::budget::AttributionQuality::Unavailable,
    }
}

// --- The RAM settle window

/// Host witness for the settle pins: one scripted shape behind a flip.
/// `held` answers the state during the window (free 20.0 of 64.0), the flip
/// answers the state after the OS returned the released pages (free 34.0).
/// Every reading is counted, so a pin can prove the window sampled the host —
/// and, for an unproven release, that it never started sampling at all.
#[derive(Default)]
struct ScriptedRamWitness {
    held: std::sync::atomic::AtomicBool,
    reads: std::sync::atomic::AtomicUsize,
}

impl ScriptedRamWitness {
    fn release_pages(&self) {
        self.held.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl crate::witness::RamWitness for ScriptedRamWitness {
    fn sample(&self) -> Option<crate::witness::RamSample> {
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(crate::witness::RamSample {
            total_gib: 64.0,
            used_gib: if self.held.load(std::sync::atomic::Ordering::Relaxed) {
                44.0
            } else {
                30.0
            },
        })
    }
}

/// Kill-confirmation runtime for the settle pins: `kill` is recorded and
/// issued, `release_confirmed` answers exactly what the pin scripts — `true`
/// for a proven release (the settle window's entry ticket), `false` forever
/// for the unproven one (falsifier 6). `spawn` is counted, never faked away,
/// so "the spawn waited" is an observed fact, not a timing guess; `probe`
/// stays pending because each pin applies its own readiness event.
#[derive(Default)]
struct SettleRuntime {
    confirm: bool,
    kills: std::sync::atomic::AtomicUsize,
    spawns: std::sync::atomic::AtomicUsize,
}

impl SettleRuntime {
    fn kills(&self) -> usize {
        self.kills.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn spawns(&self) -> usize {
        self.spawns.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for SettleRuntime {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        ticket: crate::ownership::SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        self.spawns
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(crate::engine::Spawned {
            port: ticket.port,
            pid: 9_901,
        })
    }
    async fn kill(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<(), crate::engine::SpawnError> {
        self.kills
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    async fn release_confirmed(
        &self,
        _ticket: &crate::ownership::SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        Ok(self.confirm)
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        std::future::pending::<()>().await;
        crate::engine::Probe::Ready
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        None
    }
}

/// The resident each settle pin seats: a RAM holder, a GPU holder, or a
/// caller-built slot list for the legs that need no holder.
fn ram_settle_seat(model: &str, need: f64, ram_need: Option<f64>, port: u16) -> Occupant {
    Occupant {
        model: model.to_string(),
        need: Gibibytes(need),
        ram_need: ram_need.map(Gibibytes),
        port,
        ticket: fold_ticket(port),
        loaded_at: Instant::now(),
        launched_at_wall: std::time::SystemTime::now(),
        spawn_config: None,
        stale_config: false,
        serve_id: None,
        measure: None,
        live_measured_gb: None,
    }
}

/// Drives the kill-confirm task to its release event and applies it,
/// reporting whether the release was confirmed.
async fn drive_ram_release(
    state: &mut ActorState,
    rx: &mut mpsc::UnboundedReceiver<ActorMsg>,
) -> bool {
    let msg = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the reap answers within its window")
        .expect("the actor channel is open");
    match msg {
        ActorMsg::Event { job, event } => match event {
            super::Event::Released(Ok(elapsed)) => {
                state.apply_event(job, super::Event::Released(Ok(elapsed)));
                true
            }
            super::Event::Released(Err(error)) => {
                state.apply_event(job, super::Event::Released(Err(error)));
                false
            }
            other => panic!("the reap sends its release verdict: {other:?}"),
        },
        _ => panic!("expected an event"),
    }
}

/// Takes the next actor event for this row, whatever it is, asserting its job.
async fn next_ram_event(
    rx: &mut mpsc::UnboundedReceiver<ActorMsg>,
    job_id: JobId,
    want: &str,
) -> super::Event {
    let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("expected {want}"))
        .expect("the actor channel is open");
    match msg {
        ActorMsg::Event { job, event } => {
            assert_eq!(job, job_id, "the event belongs to the settle row");
            event
        }
        other => panic!("expected {want}, got {other:?}"),
    }
}

fn fit_llama_entry(dir: &std::path::Path, name: &str) -> ModelEntry {
    ModelEntry::from_flat(
        &BTreeMap::from([
            ("name".to_string(), name.to_string()),
            ("engine".to_string(), "llama".to_string()),
            ("category".to_string(), "work".to_string()),
            ("vram_gb".to_string(), "24".to_string()),
            (
                "model".to_string(),
                dir.join("turbo.gguf").display().to_string(),
            ),
            ("context".to_string(), "4096".to_string()),
            ("ngl".to_string(), "99".to_string()),
        ]),
        name,
    )
    .expect("valid row")
}

/// The death-report pins' runtime: the death report's reap always lands. `kill`
/// records the `(port, generation)` of every ticket it reaped;
/// `release_confirmed` answers confirmed on the first poll — a dead engine's
/// refused port is the harness's own release evidence. Spawn, probe and
/// discovery never run here: these pins seat occupants directly, so any call
/// reaching them is a test bug.
#[derive(Default)]
struct DeathRuntime {
    killed: Mutex<Vec<(u16, u64)>>,
}

impl DeathRuntime {
    fn killed(&self) -> Vec<(u16, u64)> {
        self.killed.lock().expect("killed lock").clone()
    }
}

#[async_trait::async_trait]
impl crate::engine::EngineRuntime for DeathRuntime {
    async fn spawn(
        &self,
        _spec: &crate::engine::LaunchSpec,
        _ticket: SpawnTicket,
    ) -> Result<crate::engine::Spawned, crate::engine::SpawnError> {
        panic!("the RC-02 death pins reap seated occupants, never spawn");
    }
    async fn kill(&self, ticket: &SpawnTicket) -> Result<(), crate::engine::SpawnError> {
        self.killed
            .lock()
            .expect("killed lock")
            .push((ticket.port, ticket.generation));
        Ok(())
    }
    async fn release_confirmed(
        &self,
        _ticket: &SpawnTicket,
    ) -> Result<bool, crate::engine::SpawnError> {
        Ok(true)
    }
    async fn probe(
        &self,
        _port: u16,
        _readiness: &crate::engine::ReadinessSpec,
    ) -> crate::engine::Probe {
        panic!("the RC-02 death pins never probe");
    }
    async fn discover_model_id(&self, _port: u16) -> Option<String> {
        panic!("the RC-02 death pins never discover");
    }
}

/// Actor state for the death-report pins: the reap confirms immediately, and
/// the release event lands on the channel the caller drains.
fn death_state() -> (
    ActorState,
    Arc<DeathRuntime>,
    mpsc::UnboundedReceiver<ActorMsg>,
) {
    let (tx, rx) = mpsc::unbounded_channel::<ActorMsg>();
    let runtime = Arc::new(DeathRuntime::default());
    let state = ActorState::empty(
        crate::config::AppConfig::default(),
        crate::testutil::temp_dir("engine-death"),
        BTreeMap::new(),
        Arc::clone(&runtime) as Arc<dyn crate::engine::EngineRuntime>,
        Arc::new(LedgerOnlyWitness) as Arc<dyn crate::budget::VramWitness>,
        tx,
    );
    (state, runtime, rx)
}
