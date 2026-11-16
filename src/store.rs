//! Shared, refreshable portfolio content. Visitors never wait on the network.
use crate::content::{MAX_SNAPSHOT_BYTES, Portfolio};
use std::{
    sync::{Arc, Mutex, PoisonError, RwLock},
    time::Duration,
};
use tokio::{task::JoinHandle, time::Instant};

pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// Blocking fetch of a snapshot body. Runs on Tokio's blocking pool.
pub type Fetch = dyn Fn(&str) -> Result<String, String> + Send + Sync;

#[derive(Clone)]
pub struct ContentStore {
    inner: Arc<Inner>,
}

struct Inner {
    current: RwLock<Arc<Portfolio>>,
    url: Option<String>,
    fetch: Box<Fetch>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    last_attempt: Option<Instant>,
    in_flight: bool,
    last_error: Option<String>,
}

impl ContentStore {
    /// Starts from `bundled`; refreshes from `url` over HTTPS when one is configured.
    pub fn new(bundled: Portfolio, url: Option<String>) -> Self {
        Self::with_fetch(bundled, url, Box::new(fetch_https))
    }

    pub fn with_fetch(bundled: Portfolio, url: Option<String>, fetch: Box<Fetch>) -> Self {
        Self {
            inner: Arc::new(Inner {
                current: RwLock::new(Arc::new(bundled)),
                url,
                fetch,
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// The current content, immediately. May start a background refresh.
    pub fn snapshot(&self) -> Arc<Portfolio> {
        let current = self.current();
        let _ = self.refresh_if_due();
        current
    }

    fn current(&self) -> Arc<Portfolio> {
        Arc::clone(
            &self
                .inner
                .current
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Starts one background refresh when a URL is configured, none is in flight,
    /// and the last attempt was at least `REFRESH_INTERVAL` ago.
    pub fn refresh_if_due(&self) -> Option<JoinHandle<()>> {
        let url = self.inner.url.clone()?;
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        {
            let mut state = self.state();
            let now = Instant::now();
            if state.in_flight
                || state
                    .last_attempt
                    .is_some_and(|last| now.duration_since(last) < REFRESH_INTERVAL)
            {
                return None;
            }
            state.in_flight = true;
            state.last_attempt = Some(now);
        }
        let store = self.clone();
        Some(runtime.spawn_blocking(move || {
            let result = (store.inner.fetch)(&url);
            if let Some(line) = store.finish(result) {
                eprintln!("{line}");
            }
        }))
    }

    /// Applies a fetch result and returns the line to log, if any.
    fn finish(&self, result: Result<String, String>) -> Option<String> {
        let parsed = result
            .and_then(|body| {
                Portfolio::from_json(&body).map_err(|error| format!("invalid content: {error}"))
            })
            .map_err(|reason| sanitize(&reason));
        let mut state = self.state();
        state.in_flight = false;
        match parsed {
            Ok(content) => {
                state.last_error = None;
                let mut current = self
                    .inner
                    .current
                    .write()
                    .unwrap_or_else(PoisonError::into_inner);
                if **current == content {
                    return None;
                }
                *current = Arc::new(content);
                Some("content refreshed".into())
            }
            Err(reason) if state.last_error.as_ref() != Some(&reason) => {
                let line = format!("content refresh failed: {reason}; keeping last good copy");
                state.last_error = Some(reason);
                Some(line)
            }
            Err(_) => None,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Remote error text made safe for one log line: control characters become `?`,
/// and the result is at most `MAX_ERROR_CHARS` characters.
fn sanitize(text: &str) -> String {
    text.chars()
        .take(MAX_ERROR_CHARS)
        .map(|ch| if ch.is_control() { '?' } else { ch })
        .collect()
}

const MAX_ERROR_CHARS: usize = 200;

fn fetch_https(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .https_only(true)
        .http_status_as_error(false)
        .max_redirects(3)
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|error| error.to_string())?;
    if response.status() != 200 {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response
        .body_mut()
        .with_config()
        .limit(MAX_SNAPSHOT_BYTES as u64)
        .read_to_string()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    fn bundled() -> Portfolio {
        Portfolio::bundled().unwrap()
    }

    fn updated_json() -> String {
        let mut data = bundled().into_data();
        data.projects[0].summary = "Refreshed summary.".into();
        serde_json::to_string(&Portfolio::new(data).unwrap()).unwrap()
    }

    fn store_returning(
        result: impl Fn() -> Result<String, String> + Send + Sync + 'static,
    ) -> (ContentStore, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let store = ContentStore::with_fetch(
            bundled(),
            Some("https://www.example.com/portfolio.json".into()),
            Box::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
                result()
            }),
        );
        (store, calls)
    }

    #[tokio::test]
    async fn successful_refresh_swaps_content() {
        let (store, calls) = store_returning(|| Ok(updated_json()));
        let before = store.snapshot();
        // snapshot() started the first refresh; it is now in flight or finished.
        assert_eq!(before.projects[0].summary, bundled().projects[0].summary);
        while calls.load(Ordering::SeqCst) == 0 || store.state().in_flight {
            tokio::task::yield_now().await;
        }
        assert_eq!(store.current().projects[0].summary, "Refreshed summary.");
        // Earlier snapshots are untouched: sessions keep what they started with.
        assert_eq!(before.projects[0].summary, bundled().projects[0].summary);
    }

    #[tokio::test]
    async fn failed_refreshes_keep_the_last_good_copy() {
        for result in [
            Err("HTTP 404".to_string()),
            Ok("{ not json".to_string()),
            Ok(" ".repeat(MAX_SNAPSHOT_BYTES + 1)),
            Ok(updated_json().replace("\"version\":2", "\"version\":3")),
        ] {
            let (store, _) = store_returning(move || result.clone());
            store.refresh_if_due().unwrap().await.unwrap();
            assert_eq!(*store.current(), bundled());
            assert!(store.state().last_error.is_some());
        }
    }

    #[test]
    fn repeated_failures_log_once_and_recovery_resets() {
        let (store, _) = store_returning(|| Ok(String::new()));
        let failed = store.finish(Err("HTTP 404".into())).unwrap();
        assert!(failed.contains("HTTP 404"), "{failed}");
        assert_eq!(store.finish(Err("HTTP 404".into())), None);
        assert!(store.finish(Err("HTTP 500".into())).is_some());
        assert_eq!(
            store.finish(Ok(updated_json())).as_deref(),
            Some("content refreshed")
        );
        assert!(store.finish(Err("HTTP 500".into())).is_some());
    }

    #[test]
    fn an_unchanged_snapshot_is_not_logged_as_refreshed() {
        let (store, _) = store_returning(|| Ok(String::new()));
        let same = serde_json::to_string(&bundled()).unwrap();
        assert_eq!(store.finish(Ok(same.clone())), None);
        assert!(store.finish(Ok(updated_json())).is_some());
        assert_eq!(store.finish(Ok(updated_json())), None);
        // A failure in between still resets, so the next good copy is quiet if unchanged.
        assert!(store.finish(Err("HTTP 500".into())).is_some());
        assert_eq!(store.finish(Ok(updated_json())), None);
    }

    #[test]
    fn remote_error_text_is_sanitized_before_logging() {
        let (store, _) = store_returning(|| Ok(String::new()));
        let hostile = format!("bad\x1b[2J\r\nforged line\u{7}{}", "x".repeat(500));
        let line = store.finish(Err(hostile)).unwrap();
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert!(line.contains("bad?[2J??forged line?"), "{line:?}");
        let reason = line
            .strip_prefix("content refresh failed: ")
            .and_then(|rest| rest.strip_suffix("; keeping last good copy"))
            .unwrap();
        assert_eq!(reason.chars().count(), 200);
    }

    #[tokio::test(start_paused = true)]
    async fn refreshes_are_throttled_to_one_per_interval() {
        let (store, calls) = store_returning(|| Err("offline".into()));
        store.refresh_if_due().unwrap().await.unwrap();
        assert!(store.refresh_if_due().is_none());
        let _ = store.snapshot();
        tokio::time::advance(REFRESH_INTERVAL - Duration::from_secs(1)).await;
        assert!(store.refresh_if_due().is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        store.refresh_if_due().unwrap().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn refresh_never_blocks_snapshot_and_only_one_is_in_flight() {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let (store, calls) = store_returning(move || {
            gate.lock().unwrap().recv().unwrap();
            Ok(updated_json())
        });
        let refresh = store.refresh_if_due().unwrap();
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        // The fetch is blocked; snapshots still return at once with the old copy.
        let started = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(*store.snapshot(), bundled());
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(store.refresh_if_due().is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.send(()).unwrap();
        refresh.await.unwrap();
        assert_eq!(store.snapshot().projects[0].summary, "Refreshed summary.");
    }

    #[test]
    fn without_a_url_or_runtime_nothing_is_fetched() {
        let store = ContentStore::new(bundled(), None);
        assert!(store.refresh_if_due().is_none());
        let (store, calls) = store_returning(|| Ok(updated_json()));
        assert_eq!(*store.snapshot(), bundled());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
