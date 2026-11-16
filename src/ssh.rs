use crate::{app::App, content::Portfolio, input::Decoder, ui};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, layout::Rect};
use russh::{
    Channel, ChannelId, ChannelMsg, Disconnect, Pty,
    server::{self, Auth, Msg, Session},
};
use std::{
    collections::{HashMap, hash_map::Entry},
    fs::{self, OpenOptions},
    future::Future,
    io::{self, Write},
    net::{IpAddr, Ipv6Addr, Shutdown, SocketAddr},
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore, watch},
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep, sleep_until, timeout},
};

type Error = Box<dyn std::error::Error + Send + Sync>;
const ENTER: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?2004h\x1b[2J\x1b[H";
const LEAVE: &[u8] = b"\x1b]8;;\x1b\\\x1b[0m\x1b[?2004l\x1b[?25h\x1b[?1049l";
const TIME_LIMIT_MESSAGE: &[u8] = b"\r\nsession time limit reached\r\n";
const ERROR_MESSAGE: &[u8] = b"\r\nthe portfolio hit a problem, sorry; please reconnect\r\n";
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on each step of ending a connection that waits on the client: the goodbye, the
/// channel's exit status, EOF and close, the SSH disconnect, and russh winding down.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const ESCAPE_TIMEOUT: Duration = Duration::from_millis(80);
/// Keys applied from one input packet.
const MAX_KEYS: usize = 32;
/// Slack after the session deadline before the connection itself is dropped.
const DEADLINE_GRACE: Duration = Duration::from_secs(10);
/// How long a stopping server waits for each portfolio to say goodbye before disconnecting.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
/// How long a client gets to hang up by itself after its channel closes.
const HANGUP_GRACE: Duration = Duration::from_secs(1);
const RESTART_MESSAGE: &[u8] = b"\r\nthe portfolio is restarting, try again in a moment\r\n";
/// How much longer than the portfolio's idle timer russh waits before dropping a connection.
/// Before the shell starts, the setup deadline is the real limit; after it, this only has
/// to outlast the portfolio's goodbye, whose write is bounded by `CLOSE_TIMEOUT`.
const TRANSPORT_IDLE_MARGIN: Duration = Duration::from_secs(5);
/// From accept until the shell starts: version exchange, key exchange, authentication,
/// channel and PTY. OpenSSH needs well under a second; 20 s allows for slow links.
const SETUP_TIMEOUT: Duration = Duration::from_secs(20);
/// Concurrent connections from one IPv6 /56 (a typical household or site), as a multiple
/// of the per-/64 limit, so a client cannot multiply its share by hopping between /64s.
const NETWORK_FACTOR: usize = 4;

pub struct Config {
    pub bind: SocketAddr,
    pub host_key: PathBuf,
    pub max_sessions: usize,
    pub max_per_ip: usize,
    pub idle_timeout: Duration,
    pub max_session: Duration,
    /// Not a flag: tests shorten it.
    pub setup_timeout: Duration,
    pub links: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:2222".parse().unwrap(),
            host_key: "data/ssh_host_ed25519_key".into(),
            max_sessions: 32,
            max_per_ip: 3,
            idle_timeout: Duration::from_secs(300),
            max_session: Duration::from_secs(1800),
            setup_timeout: SETUP_TIMEOUT,
            links: true,
        }
    }
}

/// Serves the portfolio over anonymous SSH until Ctrl+C.
pub async fn serve(config: Config, content: Portfolio) -> Result<(), Error> {
    let key = load_or_create_key(&config.host_key)?;
    let listener = TcpListener::bind(config.bind).await?;
    let stop = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    run_server(listener, key, config, content, stop).await
}

/// Accepts visitors on `listener` until `stop` resolves, then lets each one say goodbye.
async fn run_server(
    listener: TcpListener,
    key: russh::keys::PrivateKey,
    config: Config,
    content: Portfolio,
    stop: impl Future<Output = ()>,
) -> Result<(), Error> {
    let ssh_config = Arc::new(ssh_config(key, config.idle_timeout));
    eprintln!(
        "Portfolio SSH listening on {} (maximum {} connections, {} per address)",
        listener.local_addr()?,
        config.max_sessions,
        config.max_per_ip
    );
    let content = Arc::new(content);
    let per_ip = IpLimiter::new(config.max_per_ip);
    let permits = Arc::new(Semaphore::new(config.max_sessions));
    let (shutdown, shutdown_rx) = watch::channel(false);
    let mut connections = JoinSet::new();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        // Usually descriptor exhaustion: back off instead of exiting or spinning.
                        eprintln!("accept failed: {error}");
                        sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                // Checked first so one address cannot spend global permits.
                let Some(address_slot) = per_ip.try_acquire(peer.ip()) else { continue; };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else { continue; };
                let Ok((stream, kill)) = killable(stream) else { continue; };
                let _ = stream.set_nodelay(true);
                eprintln!(
                    "session connect (active {})",
                    config.max_sessions - permits.available_permits()
                );
                let started = Instant::now();
                let deadline = started + config.max_session;
                let record = Arc::new(Mutex::new(SessionRecord::default()));
                let (stage, stage_rx) = watch::channel(Stage::Setup);
                let limits = Limits {
                    idle: config.idle_timeout,
                    deadline,
                    shutdown: shutdown_rx.clone(),
                };
                let handler = Visitor::new(
                    Arc::clone(&content),
                    limits,
                    config.links,
                    Arc::clone(&record),
                    stage,
                    Admission { _slot: address_slot, _permit: permit },
                );
                let connection = Connection {
                    kill,
                    stage: stage_rx,
                    setup_by: started + config.setup_timeout,
                    deadline,
                    shutdown: shutdown_rx.clone(),
                    record: Arc::clone(&record),
                };
                let ssh_config = Arc::clone(&ssh_config);
                connections.spawn(async move {
                    connection.run(ssh_config, stream, handler).await;
                    let record = record.lock().unwrap_or_else(PoisonError::into_inner);
                    eprintln!(
                        "session disconnect (duration {}s, size {}, reason {})",
                        started.elapsed().as_secs(),
                        record.size.map_or("none".into(), |size| format!("{}x{}", size.width, size.height)),
                        record.reason.unwrap_or(Reason::Closed).as_str(),
                    );
                });
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
            () = &mut stop => break,
        }
    }
    eprintln!("Portfolio SSH stopping ({} connections)", connections.len());
    let _ = shutdown.send(true);
    // Every connection ends within a bounded time (Connection::run), so this cannot hang.
    while connections.join_next().await.is_some() {}
    Ok(())
}

fn ssh_config(key: russh::keys::PrivateKey, idle: Duration) -> server::Config {
    server::Config {
        keys: vec![key],
        // Only a backstop for connections that never open the portfolio: the portfolio's
        // own idle timer must fire first so it can restore the visitor's screen.
        inactivity_timeout: Some(idle + TRANSPORT_IDLE_MARGIN),
        auth_rejection_time: Duration::from_millis(200),
        max_auth_attempts: 3,
        window_size: 65536,
        maximum_packet_size: 32768,
        channel_buffer_size: 16,
        event_buffer_size: 16,
        nodelay: true,
        ..Default::default()
    }
}

/// A second handle to a connection's socket. russh owns the stream and may be stuck writing
/// to a client that stopped reading; shutting the socket down makes its reads and writes
/// fail, so its session task ends and releases the connection's limits.
struct Kill(std::net::TcpStream);

impl Kill {
    fn now(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

fn killable(stream: TcpStream) -> io::Result<(TcpStream, Kill)> {
    let stream = stream.into_std()?;
    let kill = Kill(stream.try_clone()?);
    Ok((TcpStream::from_std(stream)?, kill))
}

/// How far a connection has got. The portfolio task moves it to `Ended` once it has said
/// goodbye, and the connection then closes instead of waiting for the client to hang up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Setup,
    Shell,
    Ended,
}

/// One accepted connection, from the SSH version exchange until its socket closes.
struct Connection {
    kill: Kill,
    stage: watch::Receiver<Stage>,
    /// When the shell must have started.
    setup_by: Instant,
    deadline: Instant,
    /// Becomes true when the server is stopping.
    shutdown: watch::Receiver<bool>,
    record: Arc<Mutex<SessionRecord>>,
}

impl Connection {
    /// Returns within a bounded time of the setup deadline passing before the shell starts,
    /// the portfolio ending, the server stopping, or the session deadline, whatever the
    /// client does.
    async fn run(mut self, ssh_config: Arc<server::Config>, stream: TcpStream, handler: Visitor) {
        let setup = sleep_until(self.setup_by);
        tokio::pin!(setup);
        let mut running = tokio::select! {
            result = server::run_stream(ssh_config, stream, handler) => {
                match result { Ok(running) => running, Err(_) => return }
            }
            () = &mut setup => {
                self.record.lock().unwrap_or_else(PoisonError::into_inner).reason = Some(Reason::Setup);
                return;
            }
            _ = self.shutdown.wait_for(|stopping| *stopping) => return,
        };
        let handle = running.handle();
        let message = loop {
            tokio::select! {
                _ = &mut running => return,
                // Key exchange, authentication, channel, PTY and shell share one deadline,
                // however the client paces them or keeps the transport alive meanwhile.
                // No polite disconnect: russh holds it back during key exchange, and a
                // client that has not reached the shell by now has nothing on screen to keep.
                () = &mut setup, if *self.stage.borrow() == Stage::Setup => {
                    self.record.lock().unwrap_or_else(PoisonError::into_inner).reason = Some(Reason::Setup);
                    break None;
                }
                changed = self.stage.changed() => {
                    if changed.is_err() || *self.stage.borrow() == Stage::Ended {
                        break Some("Portfolio session ended");
                    }
                }
                _ = self.shutdown.wait_for(|stopping| *stopping) => break Some("Portfolio server stopping"),
                // Backstop for connections that never reach the portfolio's own deadline.
                _ = sleep_until(self.deadline + DEADLINE_GRACE) => {
                    self.record.lock().unwrap_or_else(PoisonError::into_inner).reason.get_or_insert(Reason::Limit);
                    break Some("Session time limit reached");
                }
            }
        };
        let Some(message) = message else {
            self.kill.now();
            let _ = timeout(CLOSE_TIMEOUT, running).await;
            return;
        };
        if *self.stage.borrow() == Stage::Shell {
            // The portfolio restores the visitor's screen and says goodbye; let it finish.
            let _ = timeout(
                SHUTDOWN_GRACE,
                self.stage.wait_for(|stage| *stage == Stage::Ended),
            )
            .await;
        }
        // OpenSSH hangs up by itself once its channel closes, and then exits with the
        // portfolio's status instead of reporting our disconnect; give it a moment to.
        if *self.stage.borrow() == Stage::Ended && timeout(HANGUP_GRACE, &mut running).await.is_ok()
        {
            return;
        }
        close(&handle, &mut running, &self.kill, message).await;
    }
}

/// Ends a connection: an SSH disconnect first and, if russh cannot deliver it to a client
/// that stopped reading, the socket itself. Takes at most three `CLOSE_TIMEOUT`s.
async fn close(
    handle: &server::Handle,
    running: &mut server::RunningSession<Visitor>,
    kill: &Kill,
    message: &str,
) {
    let disconnect = handle.disconnect(Disconnect::ByApplication, message.into(), "en".into());
    let _ = timeout(CLOSE_TIMEOUT, disconnect).await;
    if timeout(CLOSE_TIMEOUT, &mut *running).await.is_err() {
        kill.now();
        let _ = timeout(CLOSE_TIMEOUT, running).await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    Quit,
    Idle,
    Limit,
    Restart,
    Closed,
    /// The shell had not started by the setup deadline.
    Setup,
    /// Rendering or writing a frame failed.
    Error,
    /// The portfolio task panicked.
    Panic,
}

impl Reason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Quit => "quit",
            Self::Idle => "idle",
            Self::Limit => "limit",
            Self::Restart => "restart",
            Self::Closed => "closed",
            Self::Setup => "setup",
            Self::Error => "error",
            Self::Panic => "panic",
        }
    }
}

/// What the disconnect log line reports. Never holds addresses or usernames.
#[derive(Default)]
struct SessionRecord {
    size: Option<Rect>,
    reason: Option<Reason>,
}

/// Concurrent connections per client address, in memory only. IPv6 peers count against
/// both their /64 (`max`) and their /56 (`max * NETWORK_FACTOR`).
#[derive(Clone)]
struct IpLimiter {
    max: usize,
    counts: Arc<Mutex<HashMap<Prefix, usize>>>,
}

/// A counted address range: the network address and its prefix length.
type Prefix = (IpAddr, u8);

struct IpSlot {
    limiter: IpLimiter,
    prefixes: Vec<Prefix>,
}

impl IpLimiter {
    fn new(max: usize) -> Self {
        Self {
            max,
            counts: Arc::default(),
        }
    }

    fn counts(&self) -> std::sync::MutexGuard<'_, HashMap<Prefix, usize>> {
        self.counts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn try_acquire(&self, ip: IpAddr) -> Option<IpSlot> {
        let length = if ip.to_canonical().is_ipv4() { 32 } else { 64 };
        let mut limits = vec![((bucket(ip), length), self.max)];
        if let Some(network) = network(ip) {
            limits.push(((network, 56), self.max * NETWORK_FACTOR));
        }
        let mut counts = self.counts();
        if limits
            .iter()
            .any(|(prefix, max)| counts.get(prefix).is_some_and(|count| count >= max))
        {
            return None;
        }
        let prefixes: Vec<Prefix> = limits.into_iter().map(|(prefix, _)| prefix).collect();
        for prefix in &prefixes {
            *counts.entry(*prefix).or_insert(0) += 1;
        }
        Some(IpSlot {
            limiter: self.clone(),
            prefixes,
        })
    }
}

/// The address a peer is counted under: IPv4 (or IPv4-mapped) as itself, IPv6 by its /64,
/// since one subscriber is usually handed a whole /64.
fn bucket(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from_bits(v6.to_bits() & !u128::from(u64::MAX))),
        v4 => v4,
    }
}

/// An IPv6 peer's /56: ISPs commonly delegate one per customer, carved into /64s.
fn network(ip: IpAddr) -> Option<IpAddr> {
    match ip.to_canonical() {
        IpAddr::V6(v6) => Some(IpAddr::V6(Ipv6Addr::from_bits(
            v6.to_bits() & !(u128::MAX >> 56),
        ))),
        IpAddr::V4(_) => None,
    }
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut counts = self.limiter.counts();
        for prefix in &self.prefixes {
            if let Entry::Occupied(mut entry) = counts.entry(*prefix) {
                *entry.get_mut() -= 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
        }
    }
}

fn load_or_create_key(path: &Path) -> Result<russh::keys::PrivateKey, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("Host key must be a regular file, not a symlink".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err("Host key must have private permissions (chmod 600)".into());
                }
            }
            return Ok(russh::keys::load_secret_key(path, None)?);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(parent)?;
    let key = russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)?;
    let encoded = key.to_openssh(russh::keys::ssh_key::LineEnding::LF)?;
    // Write a temporary file and rename it into place, so a crash never leaves a
    // truncated key that the next start would refuse to replace.
    let name = path.file_name().ok_or("Host key path must name a file")?;
    let temporary = parent.join(format!(
        ".{}.{}-{:x}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        rand::random::<u64>()
    ));
    let written = write_private(&temporary, encoded.as_bytes()).and_then(|()| {
        fs::rename(&temporary, path)?;
        sync_directory(parent)
    });
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written?;
    Ok(key)
}

/// Creates `path` (it must not exist) readable only by its owner and flushes it to disk.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Makes a rename inside `directory` durable.
fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

/// The portfolio's viewport for a client's window. 300x100 is far more than the page column
/// needs, and a full repaint at that size still fits one 64 KiB channel window; a larger
/// window keeps the portfolio in its top-left corner.
fn dimensions(cols: u32, rows: u32) -> Rect {
    Rect::new(0, 0, cols.clamp(1, 300) as u16, rows.clamp(1, 100) as u16)
}

/// A connection's share of the global and per-address limits. Owned by the russh handler,
/// so both are released only when russh drops it: after its session task really ends,
/// or when the handshake fails or times out and the handler is dropped unused.
struct Admission {
    _slot: IpSlot,
    _permit: OwnedSemaphorePermit,
}

struct Visitor {
    content: Arc<Portfolio>,
    limits: Limits,
    links: bool,
    record: Arc<Mutex<SessionRecord>>,
    channel: Option<ChannelId>,
    pty: Option<Rect>,
    started: bool,
    control: Option<watch::Sender<Option<Rect>>>,
    stage: watch::Sender<Stage>,
    task: Option<JoinHandle<()>>,
    _admission: Admission,
}

impl Visitor {
    fn new(
        content: Arc<Portfolio>,
        limits: Limits,
        links: bool,
        record: Arc<Mutex<SessionRecord>>,
        stage: watch::Sender<Stage>,
        admission: Admission,
    ) -> Self {
        Self {
            content,
            limits,
            links,
            record,
            channel: None,
            pty: None,
            started: false,
            control: None,
            stage,
            task: None,
            _admission: admission,
        }
    }

    fn reject(&self, channel: ChannelId, session: &mut Session) -> Result<(), Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    fn resize(&mut self, cols: u32, rows: u32) {
        let size = dimensions(cols, rows);
        self.pty = Some(size);
        self.record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .size = Some(size);
    }
}

impl Drop for Visitor {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl server::Handler for Visitor {
    type Error = Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Error> {
        if self.channel.is_some() {
            return Ok(());
        }
        self.channel = Some(channel.id());
        let (control, receiver) = watch::channel(None);
        self.control = Some(control);
        let content = Arc::clone(&self.content);
        let limits = self.limits.clone();
        let links = self.links;
        let record = Arc::clone(&self.record);
        let stage = self.stage.clone();
        reply.accept().await;
        self.task = Some(tokio::spawn(async move {
            run_session(channel, receiver, content, limits, links, &record).await;
            // Connection::run disconnects now, Ok or not, rather than wait for the client.
            stage.send_replace(Stage::Ended);
        }));
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _px: u32,
        _py: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Error> {
        if self.channel != Some(channel)
            || self.started
            || self.pty.is_some()
            || term.is_empty()
            || term == "dumb"
        {
            return self.reject(channel, session);
        }
        self.resize(cols, rows);
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Error> {
        if self.channel != Some(channel) || self.started || self.pty.is_none() {
            return self.reject(channel, session);
        }
        self.started = true;
        self.stage.send_replace(Stage::Shell);
        if let Some(control) = &self.control {
            let _ = control.send(self.pty);
        }
        session.channel_success(channel)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _px: u32,
        _py: u32,
        _session: &mut Session,
    ) -> Result<(), Error> {
        // Same-size changes (some terminals repeat them) would only cost a repaint.
        if self.channel == Some(channel) && self.started && self.pty != Some(dimensions(cols, rows))
        {
            self.resize(cols, rows);
            if let Some(control) = &self.control {
                let _ = control.send(self.pty);
            }
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut Session,
    ) -> Result<(), Error> {
        self.reject(channel, session)
    }
    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        _name: &str,
        session: &mut Session,
    ) -> Result<(), Error> {
        self.reject(channel, session)
    }
    async fn env_request(
        &mut self,
        channel: ChannelId,
        _name: &str,
        _value: &str,
        session: &mut Session,
    ) -> Result<(), Error> {
        self.reject(channel, session)
    }
    async fn x11_request(
        &mut self,
        channel: ChannelId,
        _single: bool,
        _protocol: &str,
        _cookie: &str,
        _screen: u32,
        session: &mut Session,
    ) -> Result<(), Error> {
        self.reject(channel, session)
    }
    async fn agent_request(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<bool, Error> {
        Ok(false)
    }
    async fn tcpip_forward(
        &mut self,
        _address: &str,
        _port: &mut u32,
        _session: &mut Session,
    ) -> Result<bool, Error> {
        Ok(false)
    }
    async fn streamlocal_forward(
        &mut self,
        _path: &str,
        _session: &mut Session,
    ) -> Result<bool, Error> {
        Ok(false)
    }
    // All direct/forwarded TCP, Unix socket, and X11 channel opens use russh's rejecting defaults.
}

/// The keys one input packet applies. A paste longer than `MAX_KEYS` keeps its first keys
/// and silently drops the rest; the session carries on. The whole packet (at most russh's
/// `maximum_packet_size`) still passes through the decoder so a long bracketed paste ends.
fn packet_keys(decoder: &mut Decoder, data: &[u8]) -> Vec<crossterm::event::KeyEvent> {
    decoder.feed_up_to(data, MAX_KEYS)
}

/// Applies a packet's keys. Every key but the last only updates state and layout; the caller
/// renders once afterwards, so a key-mash costs one frame, not one per key.
fn dispatch_keys(
    area: Rect,
    app: &mut App,
    content: &Portfolio,
    keys: Vec<crossterm::event::KeyEvent>,
) {
    let mut keys = keys.into_iter().peekable();
    while let Some(key) = keys.next() {
        app.handle_key(key);
        if app.quit {
            break;
        }
        if keys.peek().is_some() {
            // A following key may depend on wrapping/scroll bounds of the new selection.
            ui::relayout(app, content, area.width, area.height);
        }
    }
}

/// Draws one frame plus its escapes and returns the bytes to send.
fn render_frame(
    terminal: &mut Terminal<CrosstermBackend<Vec<u8>>>,
    app: &mut App,
    content: &Portfolio,
    links: bool,
) -> io::Result<Vec<u8>> {
    terminal.draw(|frame| ui::draw(frame, app, content))?;
    if links {
        terminal
            .backend_mut()
            .write_all(ui::link_escapes(app).as_bytes())?;
    }
    terminal
        .backend_mut()
        .write_all(ui::clipboard_escape(app).as_bytes())?;
    Ok(std::mem::take(terminal.backend_mut().writer_mut()))
}

#[derive(Clone)]
struct Limits {
    idle: Duration,
    deadline: Instant,
    /// Becomes true when the server is stopping.
    shutdown: watch::Receiver<bool>,
}

/// Runs the portfolio on its channel, then always restores the visitor's screen and ends the
/// channel, whether the portfolio returned, failed, or panicked. Every step that waits on
/// the client is bounded, so a client that stops reading cannot hold the task.
async fn run_session(
    mut channel: Channel<Msg>,
    control: watch::Receiver<Option<Rect>>,
    content: Arc<Portfolio>,
    limits: Limits,
    links: bool,
    record: &Mutex<SessionRecord>,
) {
    let mut entered = false;
    let portfolio = run_portfolio(&mut channel, &mut entered, control, &content, limits, links);
    let reason = match CatchUnwind(Box::pin(portfolio)).await {
        Ok(Ok(reason)) => reason,
        Ok(Err(_)) => Reason::Error,
        Err(_) => Reason::Panic,
    };
    // Record first: the task is aborted as soon as the client drops the connection.
    record.lock().unwrap_or_else(PoisonError::into_inner).reason = Some(reason);
    let farewell = farewell(entered, reason, &content.web);
    let status = u32::from(matches!(reason, Reason::Error | Reason::Panic));
    let (mut input, output) = channel.split();
    let goodbye = async {
        if !farewell.is_empty() {
            let _ = timeout(CLOSE_TIMEOUT, output.data_bytes(farewell)).await;
        }
        let _ = timeout(CLOSE_TIMEOUT, output.exit_status(status)).await;
        let _ = timeout(CLOSE_TIMEOUT, output.eof()).await;
        let _ = timeout(CLOSE_TIMEOUT, output.close()).await;
    };
    // Input left unread (see Pacer::ready) would stall russh, and the goodbye behind it.
    let discard = async {
        while input.wait().await.is_some() {}
        std::future::pending::<()>().await;
    };
    tokio::select! {
        () = goodbye => {}
        () = discard => {}
    }
}

/// Resolves to `Err` with the panic payload if polling the inner future panics, so a bug in
/// one visitor's portfolio still ends with their screen restored.
struct CatchUnwind<F>(Pin<Box<F>>);

impl<F: Future> Future for CatchUnwind<F> {
    type Output = std::thread::Result<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = self.0.as_mut();
        match panic::catch_unwind(AssertUnwindSafe(|| inner.poll(cx))) {
            Ok(poll) => poll.map(Ok),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

/// The portfolio's event loop. Sets `entered` once the alternate screen may be showing.
async fn run_portfolio(
    channel: &mut Channel<Msg>,
    entered: &mut bool,
    mut control: watch::Receiver<Option<Rect>>,
    content: &Portfolio,
    mut limits: Limits,
    links: bool,
) -> Result<Reason, Error> {
    let mut terminal: Option<Terminal<CrosstermBackend<Vec<u8>>>> = None;
    let mut area = Rect::default();
    let mut app = App::default();
    let mut decoder = Decoder::default();
    let mut pacer = Pacer::new(Instant::now());
    let mut last_input = Instant::now();
    let mut escape_deadline = None;
    let reason = loop {
        let escape_at = escape_deadline.unwrap_or_else(|| Instant::now() + limits.idle);
        let reading = pacer.ready(Instant::now());
        tokio::select! {
            biased;
            changed = control.changed() => {
                if changed.is_err() { break Reason::Closed; }
                let size = *control.borrow_and_update();
                if let Some(size) = size.filter(|size| terminal.is_none() || *size != area) {
                    let mut new_terminal = Terminal::with_options(CrosstermBackend::new(Vec::new()), TerminalOptions { viewport: Viewport::Fixed(size) })?;
                    // Carry over bytes not yet sent, such as the switch to the alternate
                    // screen when the window changes before the first frame goes out.
                    let prefix = match terminal.as_mut() {
                        None => ENTER.to_vec(),
                        Some(old) => [std::mem::take(old.backend_mut().writer_mut()).as_slice(), b"\x1b[2J\x1b[H"].concat(),
                    };
                    new_terminal.backend_mut().write_all(&prefix)?;
                    terminal = Some(new_terminal);
                    area = size;
                    *entered = true;
                    pacer.changed();
                }
            }
            _ = limits.shutdown.wait_for(|stopping| *stopping) => break Reason::Restart,
            _ = sleep_until(limits.deadline) => break Reason::Limit,
            _ = sleep_until(last_input + limits.idle) => break Reason::Idle,
            _ = sleep_until(escape_at), if escape_deadline.is_some() => {
                escape_deadline = None;
                let mut keys: Vec<_> = decoder.expire_escape().into_iter().collect();
                pacer.admit(Instant::now(), &mut keys);
                if !keys.is_empty() {
                    dispatch_keys(area, &mut app, content, keys);
                    pacer.changed();
                }
            }
            // The latest state is always drawn, at most once per FRAME_INTERVAL.
            _ = sleep_until(pacer.next_frame), if pacer.dirty => {}
            // Out of key budget: input stays unread, russh's small channel buffer fills, and
            // TCP backpressure slows the client, instead of the server decrypting a flood
            // only to drop it.
            _ = sleep_until(pacer.next_key()), if !reading => {}
            event = channel.wait(), if reading => {
                match event {
                    None | Some(ChannelMsg::Eof | ChannelMsg::Close) => break Reason::Closed,
                    Some(ChannelMsg::Data { data }) if terminal.is_some() => {
                        #[cfg(test)]
                        if data.as_ref() == tests::PANIC_INPUT {
                            panic!("injected portfolio panic");
                        }
                        last_input = Instant::now();
                        let mut keys = packet_keys(&mut decoder, &data);
                        pacer.admit(last_input, &mut keys);
                        if !keys.is_empty() {
                            dispatch_keys(area, &mut app, content, keys);
                            pacer.changed();
                        }
                        escape_deadline = decoder.has_escape().then(|| Instant::now() + ESCAPE_TIMEOUT);
                    }
                    Some(ChannelMsg::Signal { signal: russh::Sig::INT | russh::Sig::TERM }) => break Reason::Closed,
                    _ => continue,
                }
            }
        }
        if app.quit {
            break Reason::Quit;
        }
        if let Some(terminal) = &mut terminal
            && pacer.frame_due(Instant::now())
        {
            let bytes = render_frame(terminal, &mut app, content, links)?;
            timeout(WRITE_TIMEOUT, channel.data_bytes(bytes)).await??;
            pacer.drew(Instant::now());
        }
    };
    Ok(reason)
}

/// A token bucket: holds up to `burst` tokens and regains `per_second` of them.
#[derive(Clone, Copy, Debug)]
struct Bucket {
    per_second: f64,
    burst: f64,
    tokens: f64,
    updated: Instant,
}

impl Bucket {
    fn full(per_second: f64, burst: f64, now: Instant) -> Self {
        Self {
            per_second,
            burst,
            tokens: burst,
            updated: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.per_second).min(self.burst);
        self.updated = now;
    }

    /// Takes up to `wanted` whole tokens and returns how many it took.
    fn take(&mut self, now: Instant, wanted: usize) -> usize {
        self.refill(now);
        let taken = (self.tokens as usize).min(wanted);
        self.tokens -= taken as f64;
        taken
    }
}

/// Keys a session may apply: a held-down key repeats at roughly 30 per second, so 60 per
/// second with a burst of 30 never drops typing, only floods.
const KEYS_PER_SECOND: f64 = 60.0;
const KEY_BURST: f64 = 30.0;
/// At most ~30 frames per second per session.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Caps a session's CPU however fast its client sends: keys beyond the key budget are
/// dropped rather than queued, and changes are drawn at most once per `FRAME_INTERVAL`, so
/// a burst of input or of window changes becomes one frame showing the final state.
struct Pacer {
    keys: Bucket,
    /// Something changed since the last frame.
    dirty: bool,
    next_frame: Instant,
}

impl Pacer {
    fn new(now: Instant) -> Self {
        Self {
            keys: Bucket::full(KEYS_PER_SECOND, KEY_BURST, now),
            dirty: false,
            next_frame: now,
        }
    }

    /// Keeps the keys the budget allows and drops the rest.
    fn admit(&mut self, now: Instant, keys: &mut Vec<crossterm::event::KeyEvent>) {
        keys.truncate(self.keys.take(now, keys.len()));
    }

    fn changed(&mut self) {
        self.dirty = true;
    }

    fn frame_due(&self, now: Instant) -> bool {
        self.dirty && now >= self.next_frame
    }

    fn drew(&mut self, now: Instant) {
        self.dirty = false;
        self.next_frame = now + FRAME_INTERVAL;
    }

    /// Whether a key could be applied now. Until one can, input is left unread.
    fn ready(&mut self, now: Instant) -> bool {
        self.keys.refill(now);
        self.keys.tokens >= 1.0
    }

    /// When the budget next holds a whole key.
    fn next_key(&self) -> Instant {
        let missing = (1.0 - self.keys.tokens).max(0.0);
        self.keys.updated + Duration::from_secs_f64(missing / self.keys.per_second)
    }
}

/// Bytes sent as a session ends: restore the visitor's screen, then say why.
fn farewell(entered: bool, reason: Reason, web: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    if entered {
        bytes.extend_from_slice(LEAVE);
    }
    match reason {
        Reason::Quit => bytes.extend_from_slice(format!("\r\n{}\r\n", ui::goodbye(web)).as_bytes()),
        Reason::Limit => bytes.extend_from_slice(TIME_LIMIT_MESSAGE),
        Reason::Restart => bytes.extend_from_slice(RESTART_MESSAGE),
        Reason::Error | Reason::Panic => bytes.extend_from_slice(ERROR_MESSAGE),
        Reason::Idle | Reason::Closed | Reason::Setup => {}
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Page;

    /// Input that makes a test build's portfolio panic.
    pub(super) const PANIC_INPUT: &[u8] = b"\x00panic\x00";

    /// A server on an ephemeral localhost port, stopped the way a signal stops it.
    struct TestServer {
        addr: SocketAddr,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        task: JoinHandle<Result<(), Error>>,
    }

    impl TestServer {
        async fn start(config: Config) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let key =
                russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                    .unwrap();
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let content = Portfolio::bundled().unwrap();
            let stop_signal = async {
                let _ = stopped.await;
            };
            let task = tokio::spawn(run_server(listener, key, config, content, stop_signal));
            Self {
                addr,
                stop: Some(stop),
                task,
            }
        }

        /// Stops the server and returns how long it took to finish.
        async fn stop(mut self) -> Duration {
            let started = Instant::now();
            let _ = self.stop.take().unwrap().send(());
            timeout(Duration::from_secs(20), self.task)
                .await
                .expect("server did not finish stopping")
                .unwrap()
                .unwrap();
            started.elapsed()
        }
    }

    struct TestClient;

    impl russh::client::Handler for TestClient {
        type Error = russh::Error;

        async fn check_server_key(
            &mut self,
            _key: &russh::keys::PublicKeyOrCertificate,
        ) -> Result<bool, russh::Error> {
            Ok(true)
        }
    }

    /// A russh client visit. Unlike OpenSSH, russh never hangs up by itself when its channel
    /// closes, so it stands in for a stuck or hostile client: only the server can end it.
    struct Visit {
        handle: russh::client::Handle<TestClient>,
        channel: Channel<russh::client::Msg>,
        output: Vec<u8>,
    }

    impl Visit {
        async fn open(addr: SocketAddr, pty: bool) -> Self {
            let config = Arc::new(russh::client::Config::default());
            let mut handle = russh::client::connect(config, addr, TestClient)
                .await
                .unwrap();
            assert!(handle.authenticate_none("visitor").await.unwrap().success());
            let channel = handle.channel_open_session().await.unwrap();
            if pty {
                channel
                    .request_pty(false, "xterm-256color", 80, 24, 0, 0, &[])
                    .await
                    .unwrap();
            }
            channel.request_shell(false).await.unwrap();
            Self {
                handle,
                channel,
                output: Vec::new(),
            }
        }

        fn received(&self, needle: &[u8]) -> bool {
            self.output
                .windows(needle.len())
                .any(|window| window == needle)
        }

        async fn send(&self, bytes: &[u8]) {
            self.channel.data(bytes).await.unwrap();
        }

        async fn read_until(&mut self, needle: &[u8]) {
            let wanted = String::from_utf8_lossy(needle).into_owned();
            timeout(Duration::from_secs(5), async {
                while !self.received(needle) {
                    match self.channel.wait().await {
                        Some(ChannelMsg::Data { data }) => self.output.extend_from_slice(&data),
                        Some(_) => {}
                        None => panic!("channel ended before {wanted:?}"),
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("did not receive {wanted:?}"));
        }

        /// Waits until the server has dropped the connection; returns how long that took.
        async fn wait_closed(&mut self, limit: Duration) -> Duration {
            let started = Instant::now();
            timeout(limit, async {
                while let Some(message) = self.channel.wait().await {
                    if let ChannelMsg::Data { data } = message {
                        self.output.extend_from_slice(&data);
                    }
                }
                while !self.handle.is_closed() {
                    sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the server kept the connection open");
            started.elapsed()
        }
    }

    fn test_config() -> Config {
        Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn the_server_hangs_up_after_quit_even_if_the_client_does_not() {
        let server = TestServer::start(test_config()).await;
        let mut visit = Visit::open(server.addr, true).await;
        visit.read_until(ENTER).await;
        visit.send(b"q").await;
        visit.read_until(b"thanks for stopping by").await;
        visit.wait_closed(Duration::from_secs(3)).await;
        assert!(visit.received(LEAVE));
        server.stop().await;
    }

    #[tokio::test]
    async fn an_idle_session_restores_the_screen_and_hangs_up() {
        let server = TestServer::start(Config {
            idle_timeout: Duration::from_secs(1),
            ..test_config()
        })
        .await;
        let mut visit = Visit::open(server.addr, true).await;
        visit.read_until(ENTER).await;
        visit.wait_closed(Duration::from_secs(4)).await;
        assert!(visit.received(LEAVE));
        server.stop().await;
    }

    #[tokio::test]
    async fn a_panicking_portfolio_restores_the_screen_and_hangs_up() {
        let server = TestServer::start(test_config()).await;
        let mut visit = Visit::open(server.addr, true).await;
        visit.read_until(ENTER).await;
        visit.send(PANIC_INPUT).await;
        visit.read_until(ERROR_MESSAGE).await;
        let leave = visit.output.windows(LEAVE.len()).rposition(|w| w == LEAVE);
        let message = visit
            .output
            .windows(ERROR_MESSAGE.len())
            .rposition(|w| w == ERROR_MESSAGE);
        assert!(
            leave < message,
            "the screen was not restored before the message"
        );
        visit.wait_closed(Duration::from_secs(3)).await;
        // The server carries on serving other visitors.
        let mut next = Visit::open(server.addr, true).await;
        next.read_until(ENTER).await;
        server.stop().await;
    }

    #[tokio::test]
    async fn stopping_says_goodbye_and_never_waits_on_silent_clients() {
        let server = TestServer::start(test_config()).await;
        let mut visit = Visit::open(server.addr, true).await;
        visit.read_until(ENTER).await;
        // A client that connects and then says nothing at all.
        let silent = TcpStream::connect(server.addr).await.unwrap();
        sleep(Duration::from_millis(100)).await;
        let stopped = server.stop().await;
        assert!(
            stopped < Duration::from_secs(3),
            "stopping took {stopped:?}"
        );
        visit.read_until(RESTART_MESSAGE).await;
        visit.wait_closed(Duration::from_secs(1)).await;
        drop(silent);
    }

    #[test]
    fn packet_coalescing_preserves_layout_dependent_navigation() {
        let mut data = Portfolio::bundled().unwrap().into_data();
        data.projects[0].summary = "Long project description. ".repeat(60);
        let content = Portfolio::new(data).unwrap();
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(Vec::new()),
            TerminalOptions {
                viewport: Viewport::Fixed(dimensions(40, 16)),
            },
        )
        .unwrap();
        let mut app = App::default();
        terminal
            .draw(|frame| ui::draw(frame, &mut app, &content))
            .unwrap();
        terminal.backend_mut().writer_mut().clear();
        let keys = Decoder::default().feed(b"p\t\x1b[F");
        let area = terminal.get_frame().area();
        dispatch_keys(area, &mut app, &content, keys);
        assert!(app.max_scroll > 0);
        assert_eq!(app.scroll, app.max_scroll);
        terminal
            .draw(|frame| ui::draw(frame, &mut app, &content))
            .unwrap();
        assert!(terminal.backend_mut().writer_mut().len() < 65536);
    }

    #[test]
    fn a_full_key_batch_draws_the_terminal_once() {
        let content = Portfolio::bundled().unwrap();
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(Vec::new()),
            TerminalOptions {
                viewport: Viewport::Fixed(dimensions(80, 24)),
            },
        )
        .unwrap();
        let mut app = App::default();
        assert!(
            !render_frame(&mut terminal, &mut app, &content, true)
                .unwrap()
                .is_empty()
        );
        let keys = Decoder::default().feed(&b"pjjjjkkkkwjjjjkkkkthjjjjdjjkkkkj"[..]);
        assert_eq!(keys.len(), MAX_KEYS);
        let frames = terminal.get_frame().count();
        let area = terminal.get_frame().area();
        dispatch_keys(area, &mut app, &content, keys);
        assert_eq!(
            terminal.get_frame().count(),
            frames,
            "dispatch drew a frame"
        );
        assert!(terminal.backend_mut().writer_mut().is_empty());
        let bytes = render_frame(&mut terminal, &mut app, &content, true).unwrap();
        assert_eq!(terminal.get_frame().count(), frames + 1);
        assert!(!bytes.is_empty());
        assert_eq!(app.page, Page::Home);
        assert_eq!(app.theme, crate::app::Theme::Night);
    }

    #[test]
    fn a_huge_paste_is_trimmed_instead_of_ending_the_session() {
        let content = Portfolio::bundled().unwrap();
        let mut app = App::default();
        let mut decoder = Decoder::default();
        let mut paste = vec![b'j'; 40000];
        paste.push(b'q');
        let keys = packet_keys(&mut decoder, &paste);
        assert_eq!(keys.len(), MAX_KEYS);
        dispatch_keys(dimensions(80, 24), &mut app, &content, keys);
        assert!(!app.quit, "keys past the cap must be dropped, not applied");
        let keys = packet_keys(&mut decoder, b"t");
        dispatch_keys(dimensions(80, 24), &mut app, &content, keys);
        assert_eq!(app.page, Page::Thoughts);
    }

    fn admitted_visitor(limiter: &IpLimiter, permits: &Arc<Semaphore>) -> Visitor {
        let admission = Admission {
            _slot: limiter.try_acquire("192.0.2.1".parse().unwrap()).unwrap(),
            _permit: Arc::clone(permits).try_acquire_owned().unwrap(),
        };
        let limits = Limits {
            idle: Duration::from_secs(60),
            deadline: Instant::now() + Duration::from_secs(60),
            shutdown: watch::channel(false).1,
        };
        Visitor::new(
            Arc::new(Portfolio::bundled().unwrap()),
            limits,
            true,
            Arc::default(),
            watch::channel(Stage::Setup).0,
            admission,
        )
    }

    fn test_ssh_config() -> Arc<server::Config> {
        let key =
            russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .unwrap();
        Arc::new(ssh_config(key, Duration::from_secs(5)))
    }

    #[tokio::test]
    async fn limits_are_held_until_russh_drops_the_handler() {
        let limiter = IpLimiter::new(1);
        let permits = Arc::new(Semaphore::new(1));
        let released = |limiter: &IpLimiter| limiter.counts().is_empty();

        // A client that vanishes before the SSH banner: run_stream fails and drops the handler.
        let (client, server_side) = tokio::io::duplex(4096);
        drop(client);
        let visitor = admitted_visitor(&limiter, &permits);
        assert!(!released(&limiter));
        assert!(
            server::run_stream(test_ssh_config(), server_side, visitor)
                .await
                .is_err()
        );
        assert!(released(&limiter));
        assert_eq!(permits.available_permits(), 1);

        // A client past the banner: russh runs the session in its own task, and the limits
        // are held while it runs, even with our handle to it dropped, until the session ends.
        let (mut client, server_side) = tokio::io::duplex(65536);
        let visitor = admitted_visitor(&limiter, &permits);
        tokio::io::AsyncWriteExt::write_all(&mut client, b"SSH-2.0-test\r\n")
            .await
            .unwrap();
        let running = server::run_stream(test_ssh_config(), server_side, visitor)
            .await
            .unwrap();
        drop(running);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(
            !released(&limiter),
            "slot freed while the session still runs"
        );
        assert_eq!(permits.available_permits(), 0);
        drop(client);
        timeout(Duration::from_secs(5), async {
            while !released(&limiter) || permits.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("limits not released after the session ended");
    }

    #[test]
    fn per_ip_limit_counts_each_address_and_forgets_idle_ones() {
        let limiter = IpLimiter::new(2);
        let home: IpAddr = "127.0.0.1".parse().unwrap();
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        let other: IpAddr = "2001:db8::1".parse().unwrap();
        let first = limiter.try_acquire(home).unwrap();
        let second = limiter.try_acquire(mapped).unwrap();
        assert!(limiter.try_acquire(home).is_none());
        assert!(limiter.try_acquire(mapped).is_none());
        let elsewhere = limiter.try_acquire(other).unwrap();
        drop(first);
        let third = limiter.try_acquire(home).unwrap();
        drop((second, third, elsewhere));
        assert!(limiter.counts().is_empty());
    }

    #[test]
    fn ipv6_peers_share_a_bucket_per_64_prefix() {
        let v6 = |text: &str| text.parse::<IpAddr>().unwrap();
        assert_eq!(
            bucket(v6("2001:db8:1:2::1")),
            bucket(v6("2001:db8:1:2:ffff:1:2:3"))
        );
        assert_eq!(bucket(v6("2001:db8:1:2::1")), v6("2001:db8:1:2::"));
        assert_ne!(bucket(v6("2001:db8:1:2::1")), bucket(v6("2001:db8:1:3::1")));
        assert_eq!(bucket(v6("192.0.2.7")), v6("192.0.2.7"));
        assert_eq!(bucket(v6("::ffff:192.0.2.7")), v6("192.0.2.7"));
        let limiter = IpLimiter::new(1);
        let first = limiter.try_acquire(v6("2001:db8:1:2::1")).unwrap();
        assert!(limiter.try_acquire(v6("2001:db8:1:2::99")).is_none());
        let neighbour = limiter.try_acquire(v6("2001:db8:1:3::1")).unwrap();
        drop((first, neighbour));
        assert!(limiter.counts().is_empty());
    }

    #[test]
    fn ipv6_peers_cannot_multiply_their_share_by_hopping_between_64s() {
        let v6 = |text: &str| text.parse::<IpAddr>().unwrap();
        let limiter = IpLimiter::new(1);
        // Four /64s inside 2001:db8:0:100::/56 fill its share of 1 * NETWORK_FACTOR.
        let held: Vec<_> = (0..NETWORK_FACTOR)
            .map(|i| limiter.try_acquire(v6(&format!("2001:db8:0:1{i:02x}::1"))))
            .collect::<Option<_>>()
            .unwrap();
        assert!(limiter.try_acquire(v6("2001:db8:0:1ff::1")).is_none());
        let elsewhere = limiter.try_acquire(v6("2001:db8:0:200::1")).unwrap();
        // IPv4 has no network share.
        let v4: Vec<_> = (1..=8)
            .map(|i| limiter.try_acquire(v6(&format!("192.0.2.{i}"))).unwrap())
            .collect();
        drop((held, elsewhere, v4));
        assert!(limiter.counts().is_empty());
    }

    #[tokio::test]
    async fn the_shell_must_start_by_the_setup_deadline() {
        let setup = Duration::from_secs(1);
        let server = TestServer::start(Config {
            setup_timeout: setup,
            ..test_config()
        })
        .await;
        // A version line, then silence.
        let silent = async {
            let mut stream = TcpStream::connect(server.addr).await.unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut stream, b"SSH-2.0-x\r\n")
                .await
                .unwrap();
            let started = Instant::now();
            let mut sink = Vec::new();
            timeout(
                Duration::from_secs(5),
                tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut sink),
            )
            .await
            .expect("pre-auth silence kept the connection open")
            .ok();
            started.elapsed()
        };
        // Authenticated with keepalives, which reset russh's own inactivity timer, but no channel.
        let keepalive = async {
            let config = Arc::new(russh::client::Config {
                keepalive_interval: Some(Duration::from_millis(200)),
                ..Default::default()
            });
            let mut handle = russh::client::connect(config, server.addr, TestClient)
                .await
                .unwrap();
            assert!(handle.authenticate_none("visitor").await.unwrap().success());
            let started = Instant::now();
            timeout(Duration::from_secs(5), async {
                while !handle.is_closed() {
                    sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("a channel-less connection outlived the setup deadline");
            started.elapsed()
        };
        // A shell without a PTY is refused; the connection must not linger afterwards.
        let no_pty = async {
            let mut visit = Visit::open(server.addr, false).await;
            visit.wait_closed(Duration::from_secs(5)).await
        };
        let (silent, keepalive, no_pty) = tokio::join!(silent, keepalive, no_pty);
        for (case, elapsed) in [
            ("silent", silent),
            ("keepalive", keepalive),
            ("no PTY", no_pty),
        ] {
            assert!(
                elapsed < setup + Duration::from_secs(1),
                "{case}: {elapsed:?}"
            );
        }
        // A real visitor takes far less than the deadline and is not cut off by it.
        let mut visit = Visit::open(server.addr, true).await;
        visit.read_until(ENTER).await;
        sleep(setup + Duration::from_millis(500)).await;
        visit.output.clear();
        visit.send(b"w").await;
        visit.read_until(b"Copperlane").await;
        server.stop().await;
    }

    #[test]
    fn the_portfolio_idle_timer_always_beats_the_transport_timer() {
        let key =
            russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .unwrap();
        for secs in [1, 300, 3600] {
            let idle = Duration::from_secs(secs);
            let transport = ssh_config(key.clone(), idle).inactivity_timeout.unwrap();
            // Room for the goodbye after the portfolio's own idle timer fires.
            assert!(transport >= idle + 2 * CLOSE_TIMEOUT, "{secs}s");
        }
    }

    #[test]
    fn farewell_restores_the_screen_before_explaining() {
        let web = "https://www.aidanjeon.com/";
        let quit = farewell(true, Reason::Quit, web);
        assert!(quit.starts_with(LEAVE));
        assert_eq!(
            &quit[LEAVE.len()..],
            "\r\n↳ thanks for stopping by  https://www.aidanjeon.com/\r\n".as_bytes()
        );
        let limit = farewell(true, Reason::Limit, web);
        assert_eq!(&limit[LEAVE.len()..], b"\r\nsession time limit reached\r\n");
        let restart = farewell(true, Reason::Restart, web);
        assert!(restart.starts_with(LEAVE));
        assert_eq!(
            &restart[LEAVE.len()..],
            b"\r\nthe portfolio is restarting, try again in a moment\r\n"
        );
        assert_eq!(Reason::Restart.as_str(), "restart");
        assert_eq!(farewell(true, Reason::Idle, web), LEAVE);
        for reason in [Reason::Error, Reason::Panic] {
            assert_eq!(farewell(true, reason, web), [LEAVE, ERROR_MESSAGE].concat());
        }
        assert!(farewell(false, Reason::Closed, web).is_empty());
    }

    #[test]
    fn dimensions_cannot_allocate_unbounded_buffers() {
        assert_eq!(dimensions(u32::MAX, u32::MAX), Rect::new(0, 0, 300, 100));
        assert_eq!(dimensions(0, 0), Rect::new(0, 0, 1, 1));
        assert_eq!(dimensions(80, 24), Rect::new(0, 0, 80, 24));
    }

    #[test]
    fn a_full_repaint_at_the_largest_viewport_fits_one_channel_window() {
        let content = Portfolio::bundled().unwrap();
        let mut largest = 0;
        for theme in [crate::app::Theme::Paper, crate::app::Theme::Night] {
            for key in *b"hwpt?" {
                let mut terminal = Terminal::with_options(
                    CrosstermBackend::new(Vec::new()),
                    TerminalOptions {
                        viewport: Viewport::Fixed(dimensions(u32::MAX, u32::MAX)),
                    },
                )
                .unwrap();
                let mut app = App {
                    theme,
                    ..App::default()
                };
                let keys = Decoder::default().feed(&[key]);
                dispatch_keys(dimensions(u32::MAX, u32::MAX), &mut app, &content, keys);
                let bytes = render_frame(&mut terminal, &mut app, &content, true).unwrap();
                largest = largest.max(bytes.len());
            }
        }
        assert!(largest < 65536, "{largest} bytes");
    }

    #[test]
    fn a_key_flood_is_capped_and_drawn_at_a_bounded_frame_rate() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        let (mut applied, mut frames) = (0, 0);
        // A full packet of keys every millisecond for one second.
        for ms in 0..1000 {
            let now = start + Duration::from_millis(ms);
            let mut keys =
                vec![
                    crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('j'));
                    MAX_KEYS
                ];
            pacer.admit(now, &mut keys);
            if !keys.is_empty() {
                applied += keys.len();
                pacer.changed();
            }
            if pacer.frame_due(now) {
                frames += 1;
                pacer.drew(now);
            }
        }
        let burst = KEY_BURST as usize;
        let refill = KEYS_PER_SECOND as usize;
        assert!(
            (burst + refill - 2..=burst + refill).contains(&applied),
            "{applied} keys"
        );
        assert!(frames <= 31, "{frames} frames");
        // Out of budget, input is left unread until the next whole key.
        let mut fresh = Pacer::new(start);
        let mut keys =
            vec![crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('j')); MAX_KEYS];
        fresh.admit(start, &mut keys);
        assert!(!fresh.ready(start));
        let next = fresh.next_key();
        assert!(
            next > start && next <= start + Duration::from_millis(17),
            "{:?}",
            next - start
        );
        assert!(fresh.ready(next));
        // The flood's last change has been drawn, or is due within one frame interval.
        let after = start + Duration::from_millis(999) + FRAME_INTERVAL;
        assert!(!pacer.dirty || pacer.frame_due(after));
        // Ordinary typing is never dropped: a burst of ten keys after a pause.
        let later = after + Duration::from_secs(1);
        let mut keys =
            vec![crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('j')); 10];
        pacer.admit(later, &mut keys);
        assert_eq!(keys.len(), 10);
    }

    #[tokio::test]
    async fn a_key_flood_neither_ends_the_session_nor_queues_keys() {
        let server = TestServer::start(test_config()).await;
        let mut visit = Visit::open(server.addr, true).await;
        visit.read_until(ENTER).await;
        // Far more keys than the budget allows, all at once.
        for _ in 0..50 {
            visit.send(&[b'j'; MAX_KEYS]).await;
        }
        // Once the budget refills, the next key applies at once: nothing queued before it.
        sleep(Duration::from_millis(1500)).await;
        visit.output.clear();
        visit.send(b"w").await;
        visit.read_until(b"Copperlane").await;
        visit.send(b"q").await;
        visit.read_until(b"thanks for stopping by").await;
        server.stop().await;
    }

    #[test]
    fn host_key_persists_and_invalid_key_is_not_replaced() {
        let path = std::env::temp_dir().join(format!(
            "portfolio-key-test-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let first = load_or_create_key(&path).unwrap();
        let second = load_or_create_key(&path).unwrap();
        assert_eq!(first.public_key(), second.public_key());
        fs::write(&path, "invalid private key").unwrap();
        assert!(load_or_create_key(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "invalid private key");
        fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_new_host_key_lands_whole_in_a_private_directory() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "portfolio-key-dir-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let path = root.join("data").join("ssh_host_ed25519_key");
        let key = load_or_create_key(&path).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root.join("data")), 0o700);
        assert_eq!(mode(&path), 0o600);
        let names: Vec<_> = fs::read_dir(root.join("data"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            ["ssh_host_ed25519_key"],
            "temporary file left behind"
        );
        assert_eq!(
            load_or_create_key(&path).unwrap().public_key(),
            key.public_key()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
