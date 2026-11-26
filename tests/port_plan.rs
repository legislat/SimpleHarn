//! Per-fixture engine-port plans for integration tests.
//!
//! The scheduler bind-probes a slot's planned engine port before every spawn,
//! and `AppConfig::default()` plans the same fixed ports (WORK/RP/AUX/EMB from
//! 18109) for every fixture. Test targets run in parallel — separate processes,
//! and separate threads inside one process — so schedulers sharing the default
//! plan probe one number at the same instant: the first probe's bind window
//! makes every later probe refuse its own spawn honestly. Production cannot
//! hit this (one scheduler, one plan); it is purely a fixture-side collision.
//!
//! The fix is arbitration by the OS allocator: a fixture reserves its plan by
//! binding loopback port `0` and reading the allocated port back, then drops
//! the socket so the probe can bind it. While two reservation sockets are
//! alive the OS never hands the same port to both, so fixtures that reserve
//! concurrently hold disjoint plans by construction, and a candidate that some
//! namespace already claims (or that a direct-binding fixture owns) is refused
//! and retried. A fixture calls [`reserve_engine_plan`] at its start choke
//! point, where the config is final: an explicit plan must match each
//! category's slot count to be honored, so any `max_models` edit must land
//! before the reservation.

use simpleharn::config::AppConfig;

/// The static child-port band the process-runtime fixtures bind real children
/// on (per-test offsets from a fixed base, one port reserved for the lib
/// binary's engine-child test). A reserved plan port may never land there: the
/// children hold those numbers for their whole life, and a probe would then
/// refuse against a live fixture of a different suite.
#[allow(dead_code)] // this file also compiles as its own integration target, where nothing calls into it
const STATIC_CHILD_PORTS: std::ops::RangeInclusive<u16> = 18_190..=18_199;

/// Give this fixture its own engine-port plan: every category whose plan is
/// still automatic (an empty `engine_ports`) gets fresh bind-droppable loopback
/// ports, and a category that already names its own plan is left alone.
///
/// Candidates come from the OS dynamic allocator (port `0`), which never hands
/// out a number a live socket holds, never allocates a currently-excluded host
/// range, and so cannot return a port another reservation holds at that
/// instant. A candidate is refused while the reservation socket is still held
/// when this config claims the number — a protected port, a public listener,
/// or a port this plan already carries (categories are reserved in order, each
/// written back before the next is drawn) — or when it sits in
/// [`STATIC_CHILD_PORTS`]. The attempts bound is generous: hitting it means the
/// host will not hand over free loopback ports at all, and the panic says so.
#[allow(dead_code)] // this file also compiles as its own integration target, where nothing calls into it
pub fn reserve_engine_plan(config: &mut AppConfig) {
    let automatic: Vec<(usize, usize)> = config
        .categories
        .iter()
        .enumerate()
        .filter(|(_, category)| category.engine_ports.is_empty())
        .map(|(index, category)| (index, category.max_models))
        .collect();
    for (index, slots) in automatic {
        let mut ports = Vec::with_capacity(slots);
        // Held to the end of the draw so concurrent reservations inside this
        // one fixture are distinct too: the allocator skips live sockets.
        let mut reservations = Vec::with_capacity(slots);
        let mut attempts = 0usize;
        while ports.len() < slots {
            attempts += 1;
            assert!(
                attempts <= slots * 50 + 50,
                "the OS handed no bindable free loopback port for the fixture \
                 engine plan in {attempts} attempts ({} of {slots} slots \
                 reserved)",
                ports.len()
            );
            let Ok(reservation) = std::net::TcpListener::bind("127.0.0.1:0") else {
                continue;
            };
            let port = reservation
                .local_addr()
                .expect("invariant: a bound listener reports its own address")
                .port();
            if config.port_claim(port).is_some() || STATIC_CHILD_PORTS.contains(&port) {
                continue;
            }
            ports.push(port);
            reservations.push(reservation);
        }
        // The sockets die here: the probe must find the planned port bindable.
        config.categories[index].engine_ports = ports;
    }
}
