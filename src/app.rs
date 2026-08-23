use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use utility_core::config::CoreSettings;
use utility_core::logger;
use utility_core::shell::{self, Product, Shell};
use utility_core::ui::Hint;

use crate::hidpp::{self, Device, Dpi, Receiver as HidReceiver};
use crate::ui;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(flatten)]
    pub core: CoreSettings,
    /// Toast once per session when a mouse drops to this percentage while
    /// discharging. 0 disables.
    #[serde(default = "default_low_battery")]
    pub low_battery_percent: u8,
}

fn default_low_battery() -> u8 {
    20
}

impl Default for Config {
    fn default() -> Self {
        Config { core: CoreSettings::default(), low_battery_percent: default_low_battery() }
    }
}

/// Battery samples kept per device for the sparkline (one per 4 s poll).
pub const HISTORY_LEN: usize = 240;

/// Stable identity of a device across rescans: receiver pid + slot.
pub fn device_key(pid: u16, d: &Device) -> String {
    format!("{pid:04x}/{}", d.index)
}

/// Plain snapshot of one receiver and its paired devices (no handles).
#[derive(Debug, Clone)]
pub struct ReceiverInfo {
    pub pid: u16,
    pub kind: &'static str,
    pub devices: Vec<Device>,
}

pub enum AppEvent {
    Scanned(Result<Vec<ReceiverInfo>, String>),
    /// A write finished; the string describes what changed.
    Applied(Result<String, String>),
}

/// The product's own modals; Help and Update live in the shell.
pub enum Modal {
    None,
    /// Type a DPI; `←`/`→` step through onboard levels. Enter applies the snapped value.
    Dpi { input: String },
    /// Pick a report rate from the supported list.
    Rate { sel: usize },
}

pub struct App {
    pub shell: Shell,
    pub cfg: Config,
    pub modal: Modal,
    pub receivers: Vec<ReceiverInfo>,
    pub scan_error: Option<String>,
    /// Selected entry in the flattened device list.
    pub sel: usize,
    pub scanning: bool,
    /// A write is in flight; refuse another until it finishes.
    pub applying: bool,
    /// Battery percentage history per [`device_key`].
    pub history: HashMap<String, VecDeque<u8>>,
    /// Devices already warned about low battery this session.
    low_warned: HashSet<String>,
    pub last_scan: Option<Instant>,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        Self::with_shell(Shell::new(&crate::APP, ui::THEME), cfg)
    }

    /// `new` with an explicit shell (tests use [`Shell::offline`]).
    pub fn with_shell(shell: Shell, cfg: Config) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = App {
            shell,
            cfg,
            modal: Modal::None,
            receivers: Vec::new(),
            scan_error: None,
            sel: 0,
            scanning: false,
            applying: false,
            history: HashMap::new(),
            low_warned: HashSet::new(),
            last_scan: None,
            tx,
            rx,
        };
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
            AppEvent::Applied(r) => {
                self.applying = false;
                match r {
                    Ok(msg) => self.shell.set_status(msg, false),
                    Err(e) => self.shell.set_status(e, true),
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
                        self.record_batteries();
                        let n = self.devices().len();
                        self.sel = self.sel.min(n.saturating_sub(1));
                    }
                    Err(e) => self.scan_error = Some(e),
                }
            }
        }
    }

    /// Keys while no shell modal is open; `true` when consumed.
    fn product_key(&mut self, k: KeyEvent) -> bool {
        // The input modals need both the selection and the modal state, so
        // take them out of `self` first.
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::Dpi { input } => {
                self.on_key_dpi(k, input);
                return true;
            }
            Modal::Rate { sel } => {
                self.on_key_rate(k, sel);
                return true;
            }
            Modal::None => {}
        }
        let n = self.devices().len();
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if n > 0 {
                    self.sel = (self.sel + n - 1) % n;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.sel = (self.sel + 1) % n;
                }
            }
            KeyCode::Char('r') => {
                self.rescan();
                self.shell.set_status("rescanning…", false);
            }
            KeyCode::Char('d') => match self.selected() {
                Some((_, d)) if d.online && d.dpi.is_some() => {
                    let cur = d.dpi.as_ref().map(|x| x.current).unwrap_or(0);
                    self.modal = Modal::Dpi { input: cur.to_string() };
                }
                Some((_, d)) if !d.online => self.shell.set_status("device is offline — move the mouse to wake it", true),
                _ => self.shell.set_status("this device does not expose an adjustable DPI", true),
            },
            KeyCode::Char('p') => match self.selected() {
                Some((_, d)) if d.online && d.report_rate.as_ref().is_some_and(|r| !r.supported.is_empty()) => {
                    let rr = d.report_rate.as_ref().unwrap();
                    let sel = rr.supported.iter().position(|&h| h == rr.hz).unwrap_or(0);
                    self.modal = Modal::Rate { sel };
                }
                Some((_, d)) if !d.online => self.shell.set_status("device is offline — move the mouse to wake it", true),
                _ => self.shell.set_status("this device does not expose an adjustable report rate", true),
            },
            _ => return false,
        }
        true
    }

    fn on_key_dpi(&mut self, k: KeyEvent, mut input: String) {
        let sel = self.selected().map(|(ri, d)| (self.receivers[ri].pid, d.clone()));
        let dpi = sel.as_ref().and_then(|(_, d)| d.dpi.clone());
        match k.code {
            KeyCode::Esc => return,
            KeyCode::Char(c) if !c.is_ascii_digit() => {}
            KeyCode::Char(_) if input.len() >= 5 => {}
            KeyCode::Char(_) | KeyCode::Backspace => {
                shell::edit_key(&mut input, &k);
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
                        self.shell.set_status(format!("DPI is already {v}"), false);
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
                        self.shell.set_status(format!("report rate is already {hz} Hz"), false);
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

    /// Append this scan's battery readings to the history and raise the
    /// low-battery toast the first time a device crosses the threshold.
    fn record_batteries(&mut self) {
        let threshold = self.cfg.low_battery_percent;
        let notify = self.cfg.core.notify;
        let mut toasts = Vec::new();
        for r in &self.receivers {
            for d in &r.devices {
                let Some(b) = d.battery else { continue };
                let Some(p) = b.percent else { continue };
                let key = device_key(r.pid, d);
                let h = self.history.entry(key.clone()).or_default();
                h.push_back(p);
                while h.len() > HISTORY_LEN {
                    h.pop_front();
                }
                let low = threshold > 0 && p <= threshold && b.status == hidpp::ChargeState::Discharging;
                if low && self.low_warned.insert(key.clone()) {
                    let name = d.name.clone().unwrap_or_else(|| d.paired_name.clone());
                    toasts.push(format!("{name} battery {p}%"));
                } else if !low && p > threshold.saturating_add(5) {
                    // Re-arm once it has clearly recovered (charged).
                    self.low_warned.remove(&key);
                }
            }
        }
        for msg in toasts {
            self.shell.set_status(format!("low battery: {msg}"), true);
            if notify {
                std::thread::spawn(move || utility_core::notify::toast("Low battery", &msg));
            }
        }
    }

    /// Run a write against the receiver `pid` on a worker thread.
    fn start_write(&mut self, job: impl FnOnce(&HidReceiver) -> Result<String, String> + Send + 'static, pid: u16) {
        if self.applying {
            self.shell.set_status("a change is still being applied", true);
            return;
        }
        self.applying = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = hidpp::receiver(pid).and_then(|r| job(&r));
            let _ = tx.send(AppEvent::Applied(r));
        });
    }
}

impl Product for App {
    fn shell(&self) -> &Shell {
        &self.shell
    }
    fn shell_mut(&mut self) -> &mut Shell {
        &mut self.shell
    }
    fn core_settings(&self) -> &CoreSettings {
        &self.cfg.core
    }
    fn core_settings_mut(&mut self) -> &mut CoreSettings {
        &mut self.cfg.core
    }
    fn save_config(&self) -> Result<(), String> {
        utility_core::config::save(&self.cfg)
    }
    fn hints(&self) -> &[Hint] {
        ui::HINTS
    }
    fn help(&self) -> shell::Help {
        shell::Help::from_hints(ui::HINTS)
            .width(66)
            .note("Talks Logitech HID++ to Unifying / Lightspeed / Bolt receivers.")
            .note("DPI and report rate are written straight to the mouse and read back; every change is logged. A sleeping mouse shows as offline.")
    }
    fn loading(&self) -> bool {
        self.scanning && self.last_scan.is_none()
    }
    fn draw(&self, f: &mut Frame) {
        ui::draw(f, self);
    }
    fn on_tick(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            self.on_event(ev);
        }
        // Live refresh: battery / online state every few seconds.
        if !self.scanning && self.last_scan.is_some_and(|t| t.elapsed() > Duration::from_secs(4)) {
            self.rescan();
        }
    }
    fn on_key(&mut self, k: KeyEvent) -> bool {
        self.product_key(k)
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn app() -> App {
        utility_core::register(&crate::APP);
        let mut a = App::with_shell(Shell::offline(&crate::APP, ui::THEME), Config::default());
        // Pretend the first scan finished with one online mouse.
        let mut d = Device {
            index: 1,
            paired_name: "TEST MOUSE".into(),
            wpid: 0x40bd,
            kind: "mouse",
            online: true,
            protocol: Some((4, 2)),
            name: Some("TEST MOUSE".into()),
            battery: Some(hidpp::Battery { percent: Some(15), millivolts: None, status: hidpp::ChargeState::Discharging }),
            dpi: Some(Dpi { current: 1600, default: 800, presets: vec![800, 1600, 3200], ..Default::default() }),
            report_rate: Some(hidpp::ReportRate { hz: 1000, supported: vec![500, 1000] }),
            onboard_mode: Some(2),
            error: None,
        };
        d.dpi.as_mut().unwrap().segments = vec![hidpp::DpiSegment { from: 100, to: 3200, step: 50 }];
        a.cfg.core.notify = false;
        a.on_event(AppEvent::Scanned(Ok(vec![ReceiverInfo { pid: 0xc54d, kind: "Lightspeed receiver", devices: vec![d] }])));
        a.scanning = false;
        a
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn frame_renders_device() {
        let mut a = app();
        let out = shell::snapshot(&mut a, 110, 30, Duration::ZERO).unwrap();
        assert!(out.contains("TEST MOUSE"), "{out}");
        assert!(out.contains("1600"), "{out}");
        assert!(out.contains("report rate  1000 Hz"), "{out}");
    }

    #[test]
    fn low_battery_warns_once() {
        let a = app();
        assert!(a.shell.status_ref().is_some_and(|(m, e)| e && m.contains("15%")));
        assert_eq!(a.history["c54d/1"].len(), 1);
        let mut a = a;
        a.shell.clear_status();
        let rs = a.receivers.clone();
        a.on_event(AppEvent::Scanned(Ok(rs)));
        assert!(a.shell.status_ref().is_none(), "second scan must not warn again");
        assert_eq!(a.history["c54d/1"].len(), 2);
    }

    #[test]
    fn dpi_modal_keeps_q_and_snaps() {
        let mut a = app();
        shell::handle_key(&mut a, key('d'));
        assert!(matches!(a.modal, Modal::Dpi { .. }));
        shell::handle_key(&mut a, key('q'));
        assert!(!a.shell.quit, "q inside the DPI input is just ignored");
        shell::handle_key(&mut a, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(matches!(&a.modal, Modal::Dpi { input } if input == "3200"), "→ steps to the next onboard level");
        shell::handle_key(&mut a, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(a.modal, Modal::None) && !a.shell.quit);
        shell::handle_key(&mut a, key('q'));
        assert!(a.shell.quit);
    }

    #[test]
    fn rate_modal_starts_on_current() {
        let mut a = app();
        shell::handle_key(&mut a, key('p'));
        assert!(matches!(a.modal, Modal::Rate { sel: 1 }));
        let out = shell::snapshot(&mut a, 110, 30, Duration::ZERO).unwrap();
        assert!(out.contains("Set report rate"), "{out}");
    }
}
