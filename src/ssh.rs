use crate::{app::App, content::Portfolio, input::Decoder, ui};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, layout::Rect};
use russh::{
    Channel, ChannelId, ChannelMsg, Disconnect, Pty,
    server::{self, Auth, Msg, Session},
};
use std::{
    collections::{HashMap, hash_map::Entry},
    fs::{self, OpenOptions},
    io::{self, Write},
    net::{IpAddr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{OwnedSemaphorePermit, Semaphore, watch},
    task::{JoinHandle, JoinSet},
    time::{Instant, sleep, sleep_until, timeout},
};

type Error = Box<dyn std::error::Error + Send + Sync>;
const ENTER: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?2004h\x1b[2J\x1b[H";
const LEAVE: &[u8] = b"\x1b]8;;\x1b\\\x1b[0m\x1b[?2004l\x1b[?25h\x1b[?1049l";
const TIME_LIMIT_MESSAGE: &[u8] = b"\r\nsession time limit reached\r\n";
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const ESCAPE_TIMEOUT: Duration = Duration::from_millis(80);
/// Keys applied from one input packet.
const MAX_KEYS: usize = 32;
/// Slack after the session deadline before the connection itself is dropped.
const DEADLINE_GRACE: Duration = Duration::from_secs(10);
/// How long a stopping server waits for each portfolio to say goodbye before disconnecting.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
const RESTART_MESSAGE: &[u8] = b"\r\nthe portfolio is restarting, try again in a moment\r\n";
/// How much longer than the portfolio's idle timer russh waits before dropping a connection.
const TRANSPORT_IDLE_MARGIN: Duration = Duration::from_secs(10);

pub struct Config {
    pub bind: SocketAddr,
    pub host_key: PathBuf,
    pub max_sessions: usize,
    pub max_per_ip: usize,
    pub idle_timeout: Duration,
    pub max_session: Duration,
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
            links: true,
        }
    }
}

/// Shown after the alternate screen is left on a normal quit, locally and over SSH.
pub async fn serve(config: Config, content: Portfolio) -> Result<(), Error> {
    let key = load_or_create_key(&config.host_key)?;
    let ssh_config = Arc::new(ssh_config(key, config.idle_timeout));
    let listener = TcpListener::bind(config.bind).await?;
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
                let _ = stream.set_nodelay(true);
                eprintln!(
                    "session connect (active {})",
                    config.max_sessions - permits.available_permits()
                );
                let started = Instant::now();
                let deadline = started + config.max_session;
                let record = Arc::new(Mutex::new(SessionRecord::default()));
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
                    Admission { _slot: address_slot, _permit: permit },
                );
                let ssh_config = Arc::clone(&ssh_config);
                let mut shutdown_rx = shutdown_rx.clone();
                connections.spawn(async move {
                    run_connection(ssh_config, stream, handler, deadline, &mut shutdown_rx, &record).await;
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
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    let _ = shutdown.send(true);
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

async fn run_connection(
    ssh_config: Arc<server::Config>,
    stream: tokio::net::TcpStream,
    handler: Visitor,
    deadline: Instant,
    shutdown_rx: &mut watch::Receiver<bool>,
    record: &Mutex<SessionRecord>,
) {
    let mut running = tokio::select! {
        result = timeout(Duration::from_secs(10), server::run_stream(ssh_config, stream, handler)) => {
            match result { Ok(Ok(running)) => running, _ => return }
        }
        _ = shutdown_rx.changed() => return,
    };
    let handle = running.handle();
    let message = tokio::select! {
        _ = &mut running => return,
        _ = shutdown_rx.changed() => {
            // The portfolio restores the visitor's screen and says goodbye; let it finish.
            if timeout(SHUTDOWN_GRACE, &mut running).await.is_ok() {
                return;
            }
            "Portfolio server stopping"
        }
        // Backstop for connections that never reach the portfolio's own deadline.
        _ = sleep_until(deadline + DEADLINE_GRACE) => {
            record.lock().unwrap_or_else(PoisonError::into_inner).reason.get_or_insert(Reason::Limit);
            "Session time limit reached"
        }
    };
    let _ = handle
        .disconnect(Disconnect::ByApplication, message.into(), "en".into())
        .await;
    let _ = timeout(Duration::from_secs(2), running).await;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    Quit,
    Idle,
    Limit,
    Restart,
    Closed,
}

impl Reason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Quit => "quit",
            Self::Idle => "idle",
            Self::Limit => "limit",
            Self::Restart => "restart",
            Self::Closed => "closed",
        }
    }
}

/// What the disconnect log line reports. Never holds addresses or usernames.
#[derive(Default)]
struct SessionRecord {
    size: Option<Rect>,
    reason: Option<Reason>,
}

/// Concurrent connections per client address, in memory only.
#[derive(Clone)]
struct IpLimiter {
    max: usize,
    counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

struct IpSlot {
    limiter: IpLimiter,
    ip: IpAddr,
}

impl IpLimiter {
    fn new(max: usize) -> Self {
        Self {
            max,
            counts: Arc::default(),
        }
    }

    fn counts(&self) -> std::sync::MutexGuard<'_, HashMap<IpAddr, usize>> {
        self.counts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn try_acquire(&self, ip: IpAddr) -> Option<IpSlot> {
        let ip = bucket(ip);
        let mut counts = self.counts();
        let count = counts.entry(ip).or_insert(0);
        if *count >= self.max {
            return None;
        }
        *count += 1;
        Some(IpSlot {
            limiter: self.clone(),
            ip,
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

impl Drop for IpSlot {
    fn drop(&mut self) {
        if let Entry::Occupied(mut entry) = self.limiter.counts().entry(self.ip) {
            *entry.get_mut() -= 1;
            if *entry.get() == 0 {
                entry.remove();
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

fn dimensions(cols: u32, rows: u32) -> Rect {
    Rect::new(0, 0, cols.clamp(1, 400) as u16, rows.clamp(1, 200) as u16)
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
    task: Option<JoinHandle<()>>,
    _admission: Admission,
}

impl Visitor {
    fn new(
        content: Arc<Portfolio>,
        limits: Limits,
        links: bool,
        record: Arc<Mutex<SessionRecord>>,
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
        session: &mut Session,
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
        let handle = session.handle();
        reply.accept().await;
        self.task = Some(tokio::spawn(async move {
            if run_portfolio(channel, receiver, content, limits, links, &record)
                .await
                .is_err()
            {
                let _ = handle
                    .disconnect(
                        Disconnect::ByApplication,
                        "Portfolio session ended".into(),
                        "en".into(),
                    )
                    .await;
            }
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
        if self.channel == Some(channel) && self.started {
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

async fn run_portfolio(
    mut channel: Channel<Msg>,
    mut control: watch::Receiver<Option<Rect>>,
    content: Arc<Portfolio>,
    mut limits: Limits,
    links: bool,
    record: &Mutex<SessionRecord>,
) -> Result<(), Error> {
    let mut terminal = None;
    let mut app = App::default();
    let mut decoder = Decoder::default();
    let mut last_input = Instant::now();
    let mut escape_deadline = None;
    let reason = loop {
        let escape_at = escape_deadline.unwrap_or_else(|| Instant::now() + limits.idle);
        tokio::select! {
            biased;
            changed = control.changed() => {
                if changed.is_err() { break Reason::Closed; }
                let size = *control.borrow_and_update();
                if let Some(size) = size {
                    let mut new_terminal = Terminal::with_options(CrosstermBackend::new(Vec::new()), TerminalOptions { viewport: Viewport::Fixed(size) })?;
                    new_terminal.backend_mut().write_all(if terminal.is_none() { ENTER } else { b"\x1b[2J\x1b[H" })?;
                    terminal = Some(new_terminal);
                }
            }
            _ = limits.shutdown.wait_for(|stopping| *stopping) => break Reason::Restart,
            _ = sleep_until(limits.deadline) => break Reason::Limit,
            _ = sleep_until(last_input + limits.idle) => break Reason::Idle,
            _ = sleep_until(escape_at), if escape_deadline.is_some() => {
                if let Some(key) = decoder.expire_escape() { app.handle_key(key); }
                escape_deadline = None;
            }
            event = channel.wait() => {
                match event {
                    None | Some(ChannelMsg::Eof | ChannelMsg::Close) => break Reason::Closed,
                    Some(ChannelMsg::Data { data }) if terminal.is_some() => {
                        last_input = Instant::now();
                        let keys = packet_keys(&mut decoder, &data);
                        let area = terminal.as_mut().unwrap().get_frame().area();
                        dispatch_keys(area, &mut app, &content, keys);
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
        if let Some(terminal) = &mut terminal {
            let bytes = render_frame(terminal, &mut app, &content, links)?;
            timeout(WRITE_TIMEOUT, channel.data_bytes(bytes)).await??;
        }
    };
    // Record first: the task is aborted as soon as the client drops the connection.
    record.lock().unwrap_or_else(PoisonError::into_inner).reason = Some(reason);
    let farewell = farewell(terminal.is_some(), reason, &content.web);
    if !farewell.is_empty() {
        let _ = timeout(WRITE_TIMEOUT, channel.data_bytes(farewell)).await;
    }
    let _ = channel.exit_status(0).await;
    let _ = channel.eof().await;
    let _ = channel.close().await;
    Ok(())
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
        Reason::Idle | Reason::Closed => {}
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Page;

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
    fn the_portfolio_idle_timer_always_beats_the_transport_timer() {
        let key =
            russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .unwrap();
        for secs in [1, 300, 3600] {
            let idle = Duration::from_secs(secs);
            let transport = ssh_config(key.clone(), idle).inactivity_timeout.unwrap();
            assert!(transport >= idle + Duration::from_secs(10), "{secs}s");
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
        assert!(farewell(false, Reason::Closed, web).is_empty());
    }

    #[test]
    fn dimensions_cannot_allocate_unbounded_buffers() {
        assert_eq!(dimensions(u32::MAX, u32::MAX), Rect::new(0, 0, 400, 200));
        assert_eq!(dimensions(0, 0), Rect::new(0, 0, 1, 1));
        assert_eq!(dimensions(80, 24), Rect::new(0, 0, 80, 24));
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
