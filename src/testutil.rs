//! Test-support helpers shared by the crate's unit tests and its integration
//! binaries.
//!
//! A single implementation, rather than a copy per test binary, because every
//! suite had the same defect: a temp path keyed by process id alone is shared by
//! all the tests in that binary, and `#[tokio::test]` runs them concurrently, so
//! one test's `remove_dir_all` deletes another's catalog YAMLs mid-swap (flaky
//! `unknown model` failure, review 2026-10-26).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-process discriminator for minted paths, so two runs never share a dir.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Mints a directory that is unique to this call.
///
/// # Arguments
///
/// * `tag` - Short suite label so leftovers stay attributable (e.g. `wit`).
///
/// # Returns
///
/// A freshly created directory under the system temp root, emptied first if an
/// earlier crashed run left one behind. It is not deleted afterwards, so a failing
/// test can be inspected; the temp root is reclaimed by the OS.
///
/// # Panics
///
/// If the directory cannot be created.
#[doc(hidden)]
#[must_use]
pub fn temp_dir(tag: &str) -> PathBuf {
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("se-{tag}-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Test-only capture subscriber: lets a test assert that a line was really logged.
/// Production logs reach the ring through `observability`'s layer, which is installed
/// only by `main`, so without this the pin "the failure is reported" would be
/// unfalsifiable (`cargo test` runs with no subscriber at all).
///
/// This lives in the shared test support (moved out of `tray`) because the
/// capture path must be reachable in every build configuration: the tray
/// module is an optional feature, and the API pins drive the same
/// `crate::tray::testlog` path in both.
#[cfg(test)]
pub(crate) mod testlog {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tracing::Subscriber;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::Layer;

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<String>>>);

    impl Capture {
        fn push(&self, line: String) {
            self.0.lock().expect("capture lock").push(line);
        }

        fn lines(&self) -> Vec<String> {
            self.0.lock().expect("capture lock").clone()
        }
    }

    struct Recorder<'a> {
        message: &'a mut String,
        fields: &'a mut BTreeMap<String, String>,
    }

    impl<'a> tracing::field::Visit for Recorder<'a> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                *self.message = format!("{value:?}");
            } else {
                self.fields
                    .insert(field.name().to_string(), format!("{value:?}"));
            }
        }
    }

    impl<S> Layer<S> for Capture
    where
        S: Subscriber,
    {
        fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
            let mut message = String::new();
            let mut fields = BTreeMap::new();
            event.record(&mut Recorder {
                message: &mut message,
                fields: &mut fields,
            });
            let mut line = format!(
                "{} {} {}",
                event.metadata().level(),
                event.metadata().target(),
                message
            );
            for (name, value) in fields {
                line.push_str(&format!(" {name}={value}"));
            }
            self.push(line);
        }
    }

    /// A thread-local capture session; `stop` returns every line recorded so far.
    pub(crate) struct Session(Option<tracing::subscriber::DefaultGuard>, Capture);

    impl Session {
        /// Installs the capture subscriber on the calling thread. Use this (not
        /// [`with_capture`]) when the code under test must `await`: a current-thread
        /// test runtime keeps polling the future on this same thread.
        pub(crate) fn start() -> Self {
            let capture = Capture(Arc::new(Mutex::new(Vec::new())));
            let subscriber =
                tracing_subscriber::registry::Registry::default().with(capture.clone());
            Session(Some(tracing::subscriber::set_default(subscriber)), capture)
        }

        /// Uninstalls the subscriber and returns the recorded lines.
        pub(crate) fn stop(mut self) -> Vec<String> {
            drop(self.0.take());
            self.1.lines()
        }
    }

    /// Runs `run` with a subscriber that records every event it sees and returns the
    /// recorded lines alongside `run`'s value.
    pub(crate) fn with_capture<R>(run: impl FnOnce() -> R) -> (R, Vec<String>) {
        let session = Session::start();
        let result = run();
        (result, session.stop())
    }
}

#[cfg(test)]
mod tests {
    use super::temp_dir;

    #[test]
    fn consecutive_calls_never_share_a_directory() {
        // The whole point: two tests in one binary must not collide.
        let a = temp_dir("uniq");
        let b = temp_dir("uniq");
        assert_ne!(a, b);
        assert!(a.is_dir() && b.is_dir());
        std::fs::write(a.join("only-a.yaml"), "name: a\n").expect("write a");
        assert!(!b.join("only-a.yaml").exists(), "b must not see a's files");
    }
}
