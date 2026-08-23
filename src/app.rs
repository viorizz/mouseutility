use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use serde::{Deserialize, Serialize};
use utility_core::config::CoreSettings;
use utility_core::{logger, update};

use crate::hidpp::{self, Device, Dpi, Receiver as HidReceiver};
use crate::ui;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(flatten)]
    pub core: CoreSettings,
}

/// Plain snapshot of one receiver and its paired devices (no handles).
#[derive(Debug, Clone)]
pub struct ReceiverInfo {
    pub pid: u16,
    pub kind: &'static str,
    pub devices: Vec<Device>,
}

pub enum AppEvent {
    LatestRelease(Option<String>),
    Progress(String),
    Finished(Result<String, String>),
    Scanned(Result<Vec<ReceiverInfo>, String>),
    /// A write finished; the string describes what changed.
    Applied(Result<String, String>),
}

pub enum Modal {
    None,
    Help,
    Update { steps: Vec<String>, done: Option<Result<String, String>> },
    /// Type a DPI; `←`/`→` step through onboard levels. Enter applies the snapped value.
    Dpi { input: String },
    /// Pick a report rate from the supported list.
    Rate { sel: usize },
}

pub struct App {
    pub cfg: Config,
    pub modal: Modal,
    pub update: Option<String>,
    pub status: Option<(String, bool, Instant)>,
    pub tick: usize,
    pub restart_requested: bool,
    pub receivers: Vec<ReceiverInfo>,
    pub scan_error: Option<String>,
    /// Selected entry in the flattened device list.
    pub sel: usize,
    pub scanning: bool,
    /// A write is in flight; refuse another until it finishes.
    pub applying: bool,
    pub last_scan: Option<Instant>,
    quit: bool,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = App {
            cfg,
            modal: Modal::None,
            update: None,
            status: None,
            tick: 0,
            restart_requested: false,
            receivers: Vec::new(),
            scan_error: None,
            sel: 0,
            scanning: false,
            applying: false,
            last_scan: None,
            quit: false,
            tx,
            rx,
        };
        if !utility_core::cli::update_check_opted_out(&crate::APP) {
            let tx = app.tx.clone();
            std::thread::spawn(move || {
                let tag = update::check_latest().ok().flatten().map(|(t, _)| t);
                let _ = tx.send(AppEvent::LatestRelease(tag));
            });
        }
        app.rescan();
        app
    }

    /// All devices across receivers, with their receiver index.
    pub fn devices(&self) -> Vec<(usize, &Device)> {
        self.receivers.iter().enumerate().flat_map(|(i, r)| r.devices.iter().map(move |d| (i, d))).collect()
    }

    pub fn selected(&self) -> Option<(usize, &Device)> {
        self.devices().get(self.sel).copied()
    }

    /// Render one frame to an in-memory backend after the first scan (`--snapshot`).
    pub fn snapshot(&mut self, width: u16, height: u16, timeout: Duration) -> anyhow::Result<String> {
        let deadline = Instant::now() + timeout;
        while self.scanning && Instant::now() < deadline {
            if let Ok(ev) = self.rx.recv_timeout(Duration::from_millis(50)) {
                self.on_event(ev);
            }
        }
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend)?;
        terminal.draw(|f| ui::draw(f, self))?;
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..height {
            let line: String = (0..width).map(|x| buf[(x, y)].symbol()).collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
        Ok(out)
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.quit {
            terminal.draw(|f| ui::draw(f, self))?;
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press {
                        self.on_key(k);
                    }
                }
            }
            while let Ok(ev) = self.rx.try_recv() {
                self.on_event(ev);
            }
            self.tick = self.tick.wrapping_add(1);
            if self.status.as_ref().is_some_and(|(_, _, t)| t.elapsed() > Duration::from_secs(5)) {
                self.status = None;
            }
            // Live refresh: battery / online state every few seconds.
            if !self.scanning && self.last_scan.is_some_and(|t| t.elapsed() > Duration::from_secs(4)) {
                self.rescan();
            }
        }
        Ok(())
    }

    pub fn set_status(&mut self, msg: impl Into<String>, is_err: bool) {
        let msg = msg.into();
        logger::log(format!("status: {msg}"));
        self.status = Some((msg, is_err, Instant::now()));
    }

    fn rescan(&mut self) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = hidpp::receivers().map(|rs| {
                rs.iter()
                    .map(|r| ReceiverInfo { pid: r.pid, kind: r.kind(), devices: r.devices() })
                    .collect()
            });
            let _ = tx.send(AppEvent::Scanned(r));
        });
    }

    fn on_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::LatestRelease(tag) => self.update = tag,
            AppEvent::Progress(s) => {
                if let Modal::Update { steps, .. } = &mut self.modal {
                    steps.push(s);
                }
            }
            AppEvent::Finished(r) => {
                if let Modal::Update { done, .. } = &mut self.modal {
                    *done = Some(r);
                }
            }
            AppEvent::Applied(r) => {
                self.applying = false;
                match r {
                    Ok(msg) => self.set_status(msg, false),
                    Err(e) => self.set_status(e, true),
                }
                self.rescan();
            }
            AppEvent::Scanned(r) => {
                self.scanning = false;
                self.last_scan = Some(Instant::now());
                match r {
                    Ok(rs) => {
                        if self.receivers.is_empty() {
                            logger::log(format!(
                                "scan: {} receiver(s), {} device(s)",
                                rs.len(),
                                rs.iter().map(|r| r.devices.len()).sum::<usize>()
                            ));
                        }
                        self.receivers = rs;
                        self.scan_error = None;
                        let n = self.devices().len();
                        self.sel = self.sel.min(n.saturating_sub(1));
                    }
                    Err(e) => self.scan_error = Some(e),
                }
            }
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        // The input modals need both the selection and the modal state, so
        // take them out of `self` first.
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::Dpi { input } => return self.on_key_dpi(k, input),
            Modal::Rate { sel } => return self.on_key_rate(k, sel),
            other => self.modal = other,
        }
        match &mut self.modal {
            Modal::Help => self.modal = Modal::None,
            Modal::Dpi { .. } | Modal::Rate { .. } => unreachable!(),
            Modal::Update { done, .. } => match (k.code, done.as_ref()) {
                (KeyCode::Char('a'), Some(Ok(_))) => {
                    self.cfg.core.auto_update = !self.cfg.core.auto_update;
                    if let Err(e) = utility_core::config::save(&self.cfg) {
                        self.set_status(e, true);
                    }
                }
                (KeyCode::Enter, Some(Ok(msg))) if update::updated(msg) => {
                    self.restart_requested = true;
                    self.quit = true;
                }
                (KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q'), Some(_)) => self.modal = Modal::None,
                _ => {}
            },
            Modal::None => {
                let n = self.devices().len();
                match (k.code, k.modifiers) {
                    (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => self.quit = true,
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.quit = true,
                    (KeyCode::Char('?'), _) => self.modal = Modal::Help,
                    (KeyCode::Char('U'), _) => self.start_update(),
                    (KeyCode::Char('c'), _) => match logger::copy_to_clipboard() {
                        Ok(n) => self.set_status(format!("copied {n} log lines to the clipboard"), false),
                        Err(e) => self.set_status(e, true),
                    },
                    (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                        if n > 0 {
                            self.sel = (self.sel + n - 1) % n;
                        }
                    }
                    (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                        if n > 0 {
                            self.sel = (self.sel + 1) % n;
                        }
                    }
                    (KeyCode::Char('r'), _) => {
                        self.rescan();
                        self.set_status("rescanning…", false);
                    }
                    (KeyCode::Char('d'), _) => match self.selected() {
                        Some((_, d)) if d.online && d.dpi.is_some() => {
                            let cur = d.dpi.as_ref().map(|x| x.current).unwrap_or(0);
                            self.modal = Modal::Dpi { input: cur.to_string() };
                        }
                        Some((_, d)) if !d.online => self.set_status("device is offline — move the mouse to wake it", true),
                        _ => self.set_status("this device does not expose an adjustable DPI", true),
                    },
                    (KeyCode::Char('p'), _) => match self.selected() {
                        Some((_, d)) if d.online && d.report_rate.as_ref().is_some_and(|r| !r.supported.is_empty()) => {
                            let rr = d.report_rate.as_ref().unwrap();
                            let sel = rr.supported.iter().position(|&h| h == rr.hz).unwrap_or(0);
                            self.modal = Modal::Rate { sel };
                        }
                        Some((_, d)) if !d.online => self.set_status("device is offline — move the mouse to wake it", true),
                        _ => self.set_status("this device does not expose an adjustable report rate", true),
                    },
                    _ => {}
                }
            }
        }
    }

    fn on_key_dpi(&mut self, k: KeyEvent, mut input: String) {
        let sel = self.selected().map(|(ri, d)| (self.receivers[ri].pid, d.clone()));
        let dpi = sel.as_ref().and_then(|(_, d)| d.dpi.clone());
        match k.code {
            KeyCode::Esc => return,
            KeyCode::Char(c) if c.is_ascii_digit() && input.len() < 5 => input.push(c),
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Left | KeyCode::Right => {
                if let Some(dpi) = &dpi {
                    let cur: u16 = input.parse().unwrap_or(dpi.current);
                    let next = if k.code == KeyCode::Left {
                        dpi.presets.iter().rev().find(|&&p| p < cur).copied()
                    } else {
                        dpi.presets.iter().find(|&&p| p > cur).copied()
                    };
                    if let Some(n) = next {
                        input = n.to_string();
                    }
                }
            }
            KeyCode::Enter => {
                if let (Ok(v), Some(dpi), Some((pid, d))) = (input.parse::<u16>(), dpi, sel) {
                    let v = dpi.snap(v);
                    if v == dpi.current {
                        self.set_status(format!("DPI is already {v}"), false);
                        return;
                    }
                    self.start_write(
                        move |r| {
                            let mut d = d;
                            apply_dpi(r, &mut d, &dpi, v)
                        },
                        pid,
                    );
                    return;
                }
            }
            _ => {}
        }
        self.modal = Modal::Dpi { input };
    }

    fn on_key_rate(&mut self, k: KeyEvent, mut sel: usize) {
        let target = self.selected().map(|(ri, d)| (self.receivers[ri].pid, d.clone()));
        let rates = target.as_ref().and_then(|(_, d)| d.report_rate.clone()).map(|r| r.supported).unwrap_or_default();
        match k.code {
            KeyCode::Esc => return,
            KeyCode::Left | KeyCode::Up => sel = sel.saturating_sub(1),
            KeyCode::Right | KeyCode::Down => sel = (sel + 1).min(rates.len().saturating_sub(1)),
            KeyCode::Enter => {
                if let (Some(&hz), Some((pid, d))) = (rates.get(sel), target) {
                    if d.report_rate.as_ref().is_some_and(|r| r.hz == hz) {
                        self.set_status(format!("report rate is already {hz} Hz"), false);
                        return;
                    }
                    self.start_write(
                        move |r| {
                            let mut d = d;
                            apply_rate(r, &mut d, hz)
                        },
                        pid,
                    );
                    return;
                }
            }
            _ => {}
        }
        self.modal = Modal::Rate { sel };
    }

    /// Run a write against the receiver `pid` on a worker thread.
    fn start_write(&mut self, job: impl FnOnce(&HidReceiver) -> Result<String, String> + Send + 'static, pid: u16) {
        if self.applying {
            self.set_status("a change is still being applied", true);
            return;
        }
        self.applying = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = hidpp::receiver(pid).and_then(|r| job(&r));
            let _ = tx.send(AppEvent::Applied(r));
        });
    }

    fn start_update(&mut self) {
        self.modal = Modal::Update { steps: Vec::new(), done: None };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = update::self_update_with(&|s| {
                let _ = tx.send(AppEvent::Progress(s.to_string()));
            });
            let _ = tx.send(AppEvent::Finished(r));
        });
    }
}

/// Set the DPI and read it back; the message reports old → new.
pub fn apply_dpi(r: &HidReceiver, d: &mut Device, dpi: &Dpi, value: u16) -> Result<String, String> {
    let old = dpi.current;
    r.link.set_dpi(d.index, dpi, value).map_err(|e| format!("set dpi: {e}"))?;
    let now = r.link.dpi(d.index).map_err(|e| format!("read back: {e}"))?;
    logger::log(format!("dpi: slot {} {old} → {value} (device reports {})", d.index, now.current));
    if now.current != value {
        return Err(format!("device kept {} instead of {value}", now.current));
    }
    d.dpi = Some(now);
    let note = if d.onboard_mode == Some(1) { " (onboard profile active — may not persist)" } else { "" };
    Ok(format!("DPI {old} → {value}{note}"))
}

/// Set the report rate and read it back.
pub fn apply_rate(r: &HidReceiver, d: &mut Device, hz: u16) -> Result<String, String> {
    let old = d.report_rate.as_ref().map(|x| x.hz).unwrap_or(0);
    if let Some(rr) = &d.report_rate {
        if !rr.supported.is_empty() && !rr.supported.contains(&hz) {
            return Err(format!("{hz} Hz is not supported; the mouse offers {:?}", rr.supported));
        }
    }
    r.link.set_report_rate(d.index, hz).map_err(|e| format!("set report rate: {e}"))?;
    let now = r.link.report_rate(d.index).map_err(|e| format!("read back: {e}"))?;
    logger::log(format!("report rate: slot {} {old} → {hz} Hz (device reports {})", d.index, now.hz));
    if now.hz != hz {
        return Err(format!("device kept {} Hz instead of {hz} Hz", now.hz));
    }
    d.report_rate = Some(now);
    Ok(format!("report rate {old} → {hz} Hz"))
}
