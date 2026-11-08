//! The pinned survivor of the old Win32 seam. The Win32 implementations moved
//! behind the platform traits in `src/platform/`; this file keeps only the
//! descendant-walk cap because the caller-side refusal mirrors it through a
//! source tripwire (`the_descendant_cap_mirror_tracks_the_win32_walk`) that
//! reads THIS file by path. Moving the literal here would blind that pin;
//! changing or deleting it must move the tripwire in the same change.

/// The hard bound on the descendant walk's answer (`descendant_pids_of` in
/// `src/platform/windows.rs`): a healthy engine family is a handful of pids,
/// so any reachable tree this large is a pathological one and the walk stops
/// rather than spin.
///
/// FR-04: this constant is also the truncation signal. The walk NEVER returns
/// more than `DESCENDANT_CAP` pids, and the only way an answer reaches exactly
/// that length is the walk stopping at the bound (or a pathologically complete
/// tree of exactly cap size). Consumers therefore treat an answer of this
/// length as **incomplete** and must never sum or book it as complete — see
/// the refusal legs in `src/scheduler/occupant.rs`, which mirror this value
/// under a tripwire pin.
pub(crate) const DESCENDANT_CAP: usize = 4_096;
