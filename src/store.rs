//! Shared, refreshable portfolio content. Visitors never wait on the network.
use crate::content::{MAX_SNAPSHOT_BYTES, Portfolio};
use std::{
    sync::{Arc, Mutex, PoisonError, RwLock},
    time::Duration,
};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(2);
const PANICKED: &str = "refresh task panicked";

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
    in_flight: bool,
    refreshed: bool,
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

    /// The current content, immediately.
    pub fn snapshot(&self) -> Arc<Portfolio> {
        Arc::clone(
            &self
                .inner
                .current
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Refreshes once now and then every `REFRESH_INTERVAL` until `shutdown` turns true.
    /// A tick that finds the previous fetch still running is skipped.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        if self.inner.url.is_none() {
            return;
        }
        let mut ticks = interval(REFRESH_INTERVAL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticks.tick() => {
                    let _ = self.refresh();
                }
                _ = shutdown.wait_for(|stopping| *stopping) => break,
            }
        }
    }

    /// Starts one background fetch when a URL is configured and none is in flight.
    fn refresh(&self) -> Option<JoinHandle<()>> {
        let url = self.inner.url.clone()?;
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        {
            let mut state = self.state();
            if state.in_flight {
                return None;
            }
            state.in_flight = true;
        }
        // Clears `in_flight` however the task ends, including a panicking fetch.
        let guard = InFlight(self.clone());
        Some(runtime.spawn_blocking(move || {
            let guard = guard;
            let store = &guard.0;
            let result = (store.inner.fetch)(&url);
            if let Some(line) = store.finish(result) {
                eprintln!("{line}");
            }
        }))
    }

    /// Applies a fetch result and returns the line to log, if any: the first success,
    /// a recovery after a failure, a content change, or a new kind of failure.
    fn finish(&self, result: Result<String, String>) -> Option<String> {
        let parsed = result
            .and_then(|body| {
                Portfolio::from_json(&body).map_err(|error| format!("invalid content: {error}"))
            })
            .map_err(|reason| sanitize(&reason));
        let mut state = self.state();
        match parsed {
            Ok(content) => {
                let recovered = state.last_error.take().is_some();
                let first = !std::mem::replace(&mut state.refreshed, true);
                let label = content.updated.clone();
                let mut current = self
                    .inner
                    .current
                    .write()
                    .unwrap_or_else(PoisonError::into_inner);
                let changed = **current != content;
                if changed {
                    *current = Arc::new(content);
                }
                let event = match (recovered, first || changed) {
                    (false, false) => return None,
                    (false, true) => "content refreshed",
                    (true, false) => "content refresh recovered",
                    (true, true) => "content refresh recovered; content refreshed",
                };
                Some(format!(
                    "{event} ({})",
                    label.as_deref().unwrap_or("no updated label")
                ))
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

/// Marks a refresh in flight until dropped. Dropped while unwinding, it records the
/// panic as a failure (logged once), so the next tick retries and the next success
/// logs the recovery.
struct InFlight(ContentStore);

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut state = self.0.state();
        state.in_flight = false;
        if std::thread::panicking() && state.last_error.as_deref() != Some(PANICKED) {
            eprintln!("content {PANICKED}; will retry");
            state.last_error = Some(PANICKED.into());
        }
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
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };

    fn bundled() -> Portfolio {
        Portfolio::bundled().unwrap()
    }

    fn label() -> String {
        bundled().updated.clone().unwrap()
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

    /// Lets the refresh task and the blocking pool run until `count` fetches have ended.
    async fn settle(store: &ContentStore, calls: &AtomicUsize, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while calls.load(Ordering::SeqCst) < count || store.state().in_flight {
            assert!(
                std::time::Instant::now() < deadline,
                "fetch {count} never finished"
            );
            tokio::task::yield_now().await;
        }
    }

    /// Gives the refresh task a chance to act on whatever the clock just did.
    async fn yield_a_while() {
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn successful_refresh_swaps_content() {
        let (store, _) = store_returning(|| Ok(updated_json()));
        let before = store.snapshot();
        store.refresh().unwrap().await.unwrap();
        assert_eq!(store.snapshot().projects[0].summary, "Refreshed summary.");
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
            store.refresh().unwrap().await.unwrap();
            assert_eq!(*store.snapshot(), bundled());
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
            store.finish(Ok(updated_json())),
            Some(format!(
                "content refresh recovered; content refreshed ({})",
                label()
            ))
        );
        assert!(store.finish(Err("HTTP 500".into())).is_some());
    }

    #[test]
    fn success_is_logged_once_then_only_on_change_or_recovery() {
        let (store, _) = store_returning(|| Ok(String::new()));
        let same = serde_json::to_string(&bundled()).unwrap();
        let refreshed = Some(format!("content refreshed ({})", label()));
        // The first success is logged even when it matches the bundled copy.
        assert_eq!(store.finish(Ok(same.clone())), refreshed);
        assert_eq!(store.finish(Ok(same)), None);
        assert_eq!(store.finish(Ok(updated_json())), refreshed);
        assert_eq!(store.finish(Ok(updated_json())), None);
        // A recovery is logged even when the content did not change.
        assert!(store.finish(Err("HTTP 500".into())).is_some());
        assert_eq!(
            store.finish(Ok(updated_json())),
            Some(format!("content refresh recovered ({})", label()))
        );
        assert_eq!(store.finish(Ok(updated_json())), None);
        let mut unlabeled = bundled().into_data();
        unlabeled.updated = None;
        let unlabeled = Portfolio::new(unlabeled).unwrap();
        assert_eq!(
            store.finish(Ok(serde_json::to_string(&unlabeled).unwrap())),
            Some("content refreshed (no updated label)".into())
        );
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
    async fn the_refresh_task_fetches_at_startup_then_once_per_interval() {
        let (store, calls) = store_returning(|| Ok(updated_json()));
        let (shutdown, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(store.clone().run(shutdown_rx));
        settle(&store, &calls, 1).await;
        assert_eq!(store.snapshot().projects[0].summary, "Refreshed summary.");
        tokio::time::advance(REFRESH_INTERVAL - Duration::from_secs(1)).await;
        yield_a_while().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!store.state().in_flight);
        tokio::time::advance(Duration::from_secs(1)).await;
        settle(&store, &calls, 2).await;
        // Shutdown stops the task; no fetch follows.
        shutdown.send(true).unwrap();
        task.await.unwrap();
        tokio::time::advance(REFRESH_INTERVAL * 3).await;
        yield_a_while().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(!store.state().in_flight);
    }

    #[tokio::test(start_paused = true)]
    async fn a_fetch_that_outlasts_the_interval_is_never_overlapped() {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let (store, calls) = store_returning(move || {
            gate.lock().unwrap().recv().unwrap();
            Ok(updated_json())
        });
        let (shutdown, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(store.clone().run(shutdown_rx));
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        // The first fetch is blocked across three ticks; none of them starts another.
        for _ in 0..3 {
            tokio::time::advance(REFRESH_INTERVAL).await;
            yield_a_while().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(*store.snapshot(), bundled());
        release.send(()).unwrap();
        settle(&store, &calls, 1).await;
        assert_eq!(store.snapshot().projects[0].summary, "Refreshed summary.");
        // Once it ends, the next tick fetches again.
        release.send(()).unwrap();
        tokio::time::advance(REFRESH_INTERVAL).await;
        settle(&store, &calls, 2).await;
        shutdown.send(true).unwrap();
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_panicking_fetch_is_retried_on_the_next_interval() {
        let first = AtomicBool::new(true);
        let (store, calls) = store_returning(move || {
            if first.swap(false, Ordering::SeqCst) {
                panic!("injected fetch panic");
            }
            Ok(updated_json())
        });
        let (shutdown, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(store.clone().run(shutdown_rx));
        settle(&store, &calls, 1).await;
        assert_eq!(store.state().last_error.as_deref(), Some(PANICKED));
        assert_eq!(*store.snapshot(), bundled());
        tokio::time::advance(REFRESH_INTERVAL).await;
        settle(&store, &calls, 2).await;
        assert_eq!(store.snapshot().projects[0].summary, "Refreshed summary.");
        assert_eq!(store.state().last_error, None);
        shutdown.send(true).unwrap();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn refresh_never_blocks_snapshot_and_only_one_is_in_flight() {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let (store, calls) = store_returning(move || {
            gate.lock().unwrap().recv().unwrap();
            Ok(updated_json())
        });
        let refresh = store.refresh().unwrap();
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        // The fetch is blocked; snapshots still return at once with the old copy.
        let started = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(*store.snapshot(), bundled());
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(store.refresh().is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.send(()).unwrap();
        refresh.await.unwrap();
        assert_eq!(store.snapshot().projects[0].summary, "Refreshed summary.");
    }

    #[test]
    fn without_a_url_or_runtime_nothing_is_fetched() {
        let store = ContentStore::new(bundled(), None);
        assert!(store.refresh().is_none());
        let (store, calls) = store_returning(|| Ok(updated_json()));
        assert!(store.refresh().is_none());
        assert_eq!(*store.snapshot(), bundled());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn snapshots_never_fetch_and_without_a_url_the_task_exits() {
        let (store, calls) = store_returning(|| Ok(updated_json()));
        for _ in 0..10 {
            assert_eq!(*store.snapshot(), bundled());
        }
        yield_a_while().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let store = ContentStore::new(bundled(), None);
        let (_shutdown, shutdown_rx) = watch::channel(false);
        store.run(shutdown_rx).await;
    }
}
