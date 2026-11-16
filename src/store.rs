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
    fetch(&agent(FETCH_TIMEOUT, true), url)
}

/// One deadline for the whole request, no redirects (the configured URL is canonical,
/// so a 3xx is an error) and no proxy, whatever HTTP(S)_PROXY or ALL_PROXY say.
fn agent(timeout: Duration, https_only: bool) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .https_only(https_only)
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .build()
        .into()
}

fn fetch(agent: &ureq::Agent, url: &str) -> Result<String, String> {
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
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        thread,
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

    /// A local plain-HTTP server: reads each request head, counts it and hands the
    /// connection to `respond`. Returns the base URL and the request counter.
    fn local_server(
        respond: impl Fn(&mut TcpStream) + Send + 'static,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut head = BufReader::new(&stream);
                let mut line = String::new();
                while head.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" {
                    line.clear();
                }
                counter.fetch_add(1, Ordering::SeqCst);
                respond(&mut stream);
            }
        });
        (base, requests)
    }

    fn reply(status: &str, headers: &str, body: &str) -> impl Fn(&mut TcpStream) + use<> {
        let response = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        move |stream| {
            let _ = stream.write_all(response.as_bytes());
        }
    }

    /// Keeps the connection open, sending nothing more, until the client hangs up.
    fn hold(stream: &mut TcpStream) {
        let _ = stream.read_to_end(&mut Vec::new());
    }

    fn test_agent(timeout: Duration) -> ureq::Agent {
        agent(timeout, false)
    }

    /// A proxy for the child process below; nothing listens on the discard port.
    const PROXY_ENV: (&str, &str) = ("ALL_PROXY", "http://127.0.0.1:9");

    fn assert_no_proxy_or_redirects() {
        for https_only in [true, false] {
            let agent = agent(FETCH_TIMEOUT, https_only);
            let config = agent.config();
            assert!(config.proxy().is_none(), "the fetch agent uses a proxy");
            assert_eq!(
                config.max_redirects(),
                0,
                "the fetch agent follows redirects"
            );
        }
    }

    #[test]
    #[ignore = "run by fetch_agent_uses_no_proxy_and_follows_no_redirects, with a proxy set"]
    fn fetch_agent_config_with_a_proxy_in_the_environment() {
        assert_eq!(std::env::var(PROXY_ENV.0).as_deref(), Ok(PROXY_ENV.1));
        assert_no_proxy_or_redirects();
    }

    #[test]
    fn fetch_agent_uses_no_proxy_and_follows_no_redirects() {
        assert_no_proxy_or_redirects();
        // ureq takes its default proxy from ALL_PROXY and friends when it builds a config, so
        // check again with one set. Setting it here would race other tests reading the
        // environment, so a child process runs the check.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::tests::fetch_agent_config_with_a_proxy_in_the_environment",
                "--ignored",
                "--test-threads=1",
            ])
            .env(PROXY_ENV.0, PROXY_ENV.1)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn fetch_returns_a_good_body() {
        let body = updated_json();
        let (base, _) = local_server(reply("200 OK", "", &body));
        let agent = test_agent(FETCH_TIMEOUT);
        assert_eq!(fetch(&agent, &format!("{base}/portfolio.json")), Ok(body));
    }

    #[test]
    fn fetch_treats_a_404_as_an_error() {
        let (base, _) = local_server(reply("404 Not Found", "", "missing"));
        let agent = test_agent(FETCH_TIMEOUT);
        assert_eq!(
            fetch(&agent, &format!("{base}/portfolio.json")),
            Err("HTTP 404".into())
        );
    }

    #[test]
    fn fetch_does_not_follow_redirects() {
        let (base, requests) =
            local_server(reply("302 Found", "Location: /elsewhere.json\r\n", ""));
        let agent = test_agent(FETCH_TIMEOUT);
        assert_eq!(
            fetch(&agent, &format!("{base}/portfolio.json")),
            Err("HTTP 302".into())
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fetch_cuts_off_an_oversized_body_while_streaming() {
        // Chunks past the 1 MiB cap with no length up front and no end: only the cap,
        // not the server finishing or the deadline, can end this read.
        let (base, _) = local_server(|stream| {
            let chunk = format!("10000\r\n{}\r\n", " ".repeat(0x10000));
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
            for _ in 0..=MAX_SNAPSHOT_BYTES / 0x10000 {
                if stream.write_all(chunk.as_bytes()).is_err() {
                    return;
                }
            }
            hold(stream);
        });
        let agent = test_agent(Duration::from_secs(30));
        let started = std::time::Instant::now();
        let error = fetch(&agent, &format!("{base}/portfolio.json")).unwrap_err();
        assert!(error.contains("larger than request limit"), "{error}");
        assert!(started.elapsed() < FETCH_TIMEOUT, "{:?}", started.elapsed());
    }

    #[test]
    fn fetch_gives_up_on_a_stalled_server_within_the_deadline() {
        let deadline = Duration::from_millis(300);
        let stalls: [fn(&mut TcpStream); 2] = [
            // Never answers.
            hold,
            // Starts the body, then stops.
            |stream| {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{");
                hold(stream);
            },
        ];
        for stall in stalls {
            let (base, _) = local_server(stall);
            let agent = test_agent(deadline);
            let started = std::time::Instant::now();
            let error = fetch(&agent, &format!("{base}/portfolio.json")).unwrap_err();
            let elapsed = started.elapsed();
            assert!(error.contains("timeout"), "{error}");
            assert!(elapsed >= deadline && elapsed < deadline * 4, "{elapsed:?}");
        }
    }
}
