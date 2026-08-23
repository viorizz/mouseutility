use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use serde::{Deserialize, Serialize};
use utility_core::config::CoreSettings;
use utility_core::{logger, update};

use crate::hidpp::{self, Device};
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
}

pub enum Modal {
    None,
    Help,
    Update { steps: Vec<String>, done: Option<Result<String, String>> },
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
        match &mut self.modal {
            Modal::Help => self.modal = Modal::None,
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
                    _ => {}
                }
            }
        }
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
