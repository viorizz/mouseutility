//! Logitech HID++ over `hidapi`.
//!
//! A receiver (Unifying / Lightspeed / Bolt) exposes two vendor collections on
//! usage page 0xFF00: usage 1 carries short reports (0x10, 7 bytes), usage 2
//! long reports (0x11, 20 bytes). On Windows each collection is its own
//! handle, and a reply always arrives on the collection matching *its own*
//! length — so a short request can get a long answer on the other handle.
//! [`Link`] owns both and reads whichever answers.
//!
//! The receiver itself (device index 0xFF) speaks HID++ 1.0 registers; paired
//! devices (index 1..=6) speak HID++ 2.0 features. A device plugged in
//! directly (USB cable, vendor page 0xFF00 on a non-receiver PID) or paired
//! over Bluetooth (vendor page 0xFF43, long reports only) is addressed at
//! index 0xFF itself and listed as its own "transport" entry — that path is
//! untested here (no such device on the development machine) and read-only. A paired device that is
//! asleep or switched off answers the ping with HID++ 1.0 error 0x08
//! ("unknown device") — reported here as `online = false`, not as a failure.
//!
//! Writes (`set_dpi`, `set_report_rate`, `set_current_dpi_index`) were
//! verified on a G PRO X2 SUPERSTRIKE over Lightspeed: `0x2202` fn 6 takes
//! `sensor, dpiX, dpiY, lod`; `0x8061` fn 3 takes the rate code as its *first*
//! byte and applies it to the active (wireless) link — the connection-type
//! byte used by the getters is not the first parameter there.
//!
//! Editing the *stored* onboard profile is deliberately absent: the X2's
//! firmware accepts the documented `0x8100` startWrite / writeData sequence
//! but always fails endWrite with error 0x04 (tried counts 4–0x1000, both
//! onboard and host mode, with and without G HUB running), so profile memory
//! is treated as read-only here.

use std::time::Duration;

use hidapi::{HidApi, HidDevice, HidError};

pub const LOGITECH: u16 = 0x046d;
const SWID: u8 = 0x0a;
const TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Debug, thiserror::Error)]
pub enum HidppError {
    #[error("hid: {0}")]
    Hid(#[from] HidError),
    #[error("no reply")]
    Timeout,
    /// HID++ 1.0 error code from the receiver (0x08 = device offline).
    #[error("hid++ 1.0 error {0:#04x}")]
    V1(u8),
    /// HID++ 2.0 error code from the device.
    #[error("hid++ 2.0 error {0:#04x}")]
    V2(u8),
    #[error("feature {0:#06x} not supported")]
    NoFeature(u16),
}

pub type Result<T> = std::result::Result<T, HidppError>;

/// The pair of vendor collections of one receiver / device.
pub struct Link {
    short: Option<HidDevice>,
    long: Option<HidDevice>,
}

impl Link {
    fn write(&self, report: &[u8]) -> Result<()> {
        let dev = if report[0] == 0x10 { self.short.as_ref().or(self.long.as_ref()) } else { self.long.as_ref().or(self.short.as_ref()) };
        let dev = dev.ok_or(HidppError::Timeout)?;
        dev.write(report)?;
        Ok(())
    }

    /// Wait for the reply to (`idx`, `sub`, `addr`) on either collection.
    /// Notifications and unrelated replies are skipped.
    fn read_reply(&self, idx: u8, sub: u8, addr: u8) -> Result<Vec<u8>> {
        let deadline = std::time::Instant::now() + TIMEOUT * 3;
        while std::time::Instant::now() < deadline {
            for dev in [&self.short, &self.long].into_iter().flatten() {
                let mut buf = [0u8; 64];
                let n = dev.read_timeout(&mut buf, 60)?;
                if n < 4 || buf[1] != idx {
                    continue;
                }
                // HID++ 1.0 error: [10, idx, 8f, sub, addr, err]
                if buf[2] == 0x8f && buf[3] == sub && buf[4] == addr {
                    return Err(HidppError::V1(buf[5]));
                }
                // HID++ 2.0 error: [11, idx, ff, feat, fn|sw, err]
                if buf[2] == 0xff && buf[3] == sub && buf[4] == addr {
                    return Err(HidppError::V2(buf[5]));
                }
                if buf[2] == sub && buf[3] == addr {
                    return Ok(buf[..n].to_vec());
                }
            }
        }
        Err(HidppError::Timeout)
    }

    /// HID++ 2.0 feature call. Returns the 16 parameter bytes.
    pub fn call(&self, idx: u8, feat: u8, func: u8, params: &[u8]) -> Result<[u8; 16]> {
        let mut req = [0u8; 20];
        req[0] = 0x11;
        req[1] = idx;
        req[2] = feat;
        req[3] = (func << 4) | SWID;
        req[4..4 + params.len()].copy_from_slice(params);
        self.write(&req)?;
        let r = self.read_reply(idx, feat, (func << 4) | SWID)?;
        let mut out = [0u8; 16];
        let n = r.len().min(20).saturating_sub(4);
        out[..n].copy_from_slice(&r[4..4 + n]);
        Ok(out)
    }

    /// HID++ 1.0 register read on the receiver (`sub` 0x81 short / 0x83 long).
    pub fn register(&self, sub: u8, reg: u8, params: &[u8]) -> Result<Vec<u8>> {
        let mut req = [0x10u8, 0xff, sub, reg, 0, 0, 0];
        req[4..4 + params.len()].copy_from_slice(params);
        self.write(&req)?;
        self.read_reply(0xff, sub, reg)
    }

    /// Root.getProtocolVersion. `Ok(None)` when the device is paired but
    /// offline; `Err` for real failures.
    pub fn ping(&self, idx: u8) -> Result<Option<(u8, u8)>> {
        match self.call(idx, 0x00, 1, &[0, 0, 0x55]) {
            Ok(p) => Ok(Some((p[0], p[1]))),
            Err(HidppError::V1(0x08)) | Err(HidppError::V1(0x09)) => Ok(None),
            Err(HidppError::Timeout) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn feature_index(&self, idx: u8, feature: u16) -> Result<u8> {
        let r = self.call(idx, 0x00, 0, &[(feature >> 8) as u8, feature as u8])?;
        if r[0] == 0 { Err(HidppError::NoFeature(feature)) } else { Ok(r[0]) }
    }

    pub fn device_name(&self, idx: u8) -> Result<String> {
        let f = self.feature_index(idx, 0x0005)?;
        let count = self.call(idx, f, 0, &[])?[0] as usize;
        let mut name = Vec::new();
        while name.len() < count {
            let r = self.call(idx, f, 1, &[name.len() as u8])?;
            let take = (count - name.len()).min(16);
            name.extend_from_slice(&r[..take]);
        }
        Ok(String::from_utf8_lossy(&name).trim_end_matches('\0').trim().to_string())
    }

    /// 0x0005 getDeviceType, for devices without a pairing-table entry.
    pub fn device_type(&self, idx: u8) -> Result<&'static str> {
        let f = self.feature_index(idx, 0x0005)?;
        let r = self.call(idx, f, 2, &[])?;
        Ok(match r[0] {
            0 => "keyboard",
            1 => "remote",
            2 => "numpad",
            3 => "mouse",
            4 => "touchpad",
            5 => "trackball",
            6 => "gamepad",
            7 => "joystick",
            8 => "presenter",
            9 => "receiver",
            10 => "headset",
            11 => "webcam",
            12 => "steering wheel",
            _ => "device",
        })
    }

    pub fn battery(&self, idx: u8) -> Result<Battery> {
        if let Ok(f) = self.feature_index(idx, 0x1004) {
            let r = self.call(idx, f, 1, &[])?;
            let status = match r[2] {
                0 => ChargeState::Discharging,
                1 | 2 => ChargeState::Charging,
                3 => ChargeState::Full,
                _ => ChargeState::Unknown,
            };
            return Ok(Battery { percent: Some(r[0]), millivolts: None, status });
        }
        if let Ok(f) = self.feature_index(idx, 0x1000) {
            let r = self.call(idx, f, 0, &[])?;
            let status = match r[2] {
                0 => ChargeState::Discharging,
                1 | 2 | 4 | 5 => ChargeState::Charging,
                3 => ChargeState::Full,
                _ => ChargeState::Unknown,
            };
            return Ok(Battery { percent: Some(r[0]), millivolts: None, status });
        }
        if let Ok(f) = self.feature_index(idx, 0x1001) {
            let r = self.call(idx, f, 0, &[])?;
            let mv = u16::from_be_bytes([r[0], r[1]]);
            let status = if r[2] & 0x80 != 0 { ChargeState::Charging } else { ChargeState::Discharging };
            return Ok(Battery { percent: None, millivolts: Some(mv), status });
        }
        Err(HidppError::NoFeature(0x1000))
    }

    pub fn dpi(&self, idx: u8) -> Result<Dpi> {
        if let Ok(f) = self.feature_index(idx, 0x2202) {
            return self.dpi_extended(idx, f);
        }
        let f = self.feature_index(idx, 0x2201)?;
        let r = self.call(idx, f, 2, &[0])?;
        let current = u16::from_be_bytes([r[1], r[2]]);
        let default = u16::from_be_bytes([r[3], r[4]]);
        let l = self.call(idx, f, 1, &[0])?;
        Ok(Dpi { current, default, segments: parse_dpi_list(&l[1..]), presets: Vec::new(), lod: 0 })
    }

    /// 0x2202 Extended Adjustable DPI: getSensorDpiParameters (fn 5) →
    /// sensor, dpiX, defaultX, dpiY, defaultY, lod; getSensorDpiRanges (fn 2:
    /// sensor, direction, index) → list of u16 where 0xE000|n encodes the step
    /// between the surrounding values; getSensorDpiList (fn 3) → preset levels.
    fn dpi_extended(&self, idx: u8, f: u8) -> Result<Dpi> {
        let r = self.call(idx, f, 5, &[0])?;
        let current = u16::from_be_bytes([r[1], r[2]]);
        let default = u16::from_be_bytes([r[3], r[4]]);
        let lod = r[9];
        // The range list is one stream of big-endian u16 split across
        // successive replies (13 payload bytes each), terminated by 0x0000.
        let mut stream = Vec::new();
        for range_idx in 0..8u8 {
            let Ok(l) = self.call(idx, f, 2, &[0, 0, range_idx]) else { break };
            stream.extend_from_slice(&l[3..16]);
            if l[3..16].chunks(2).any(|c| c.len() == 2 && c[0] == 0 && c[1] == 0) {
                break;
            }
        }
        let presets = match self.call(idx, f, 3, &[0, 0]) {
            Ok(l) => l[2..16]
                .chunks(2)
                .filter(|c| c.len() == 2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .take_while(|&v| v != 0)
                .collect(),
            Err(_) => Vec::new(),
        };
        Ok(Dpi { current, default, segments: parse_dpi_list(&stream), presets, lod })
    }

    /// Write a new DPI (both axes). `value` should already be snapped with
    /// [`Dpi::snap`]; the device rejects unsupported values with error 0x02.
    pub fn set_dpi(&self, idx: u8, dpi: &Dpi, value: u16) -> Result<()> {
        let [hi, lo] = value.to_be_bytes();
        if let Ok(f) = self.feature_index(idx, 0x2202) {
            self.call(idx, f, 6, &[0, hi, lo, hi, lo, dpi.lod])?;
            return Ok(());
        }
        let f = self.feature_index(idx, 0x2201)?;
        self.call(idx, f, 3, &[0, hi, lo])?;
        Ok(())
    }

    /// Report rate (0x8061 extended or 0x8060 legacy). On 0x8061 the active
    /// wireless link (connection type 1) is read first, wired (0) as fallback.
    pub fn report_rate(&self, idx: u8) -> Result<ReportRate> {
        if let Ok(f) = self.feature_index(idx, 0x8061) {
            let (ct, r) = match self.call(idx, f, 2, &[1]) {
                Ok(r) => (1u8, r),
                Err(_) => (0u8, self.call(idx, f, 2, &[0])?),
            };
            // Reply: connection type echo, then a bitmask of supported rate codes.
            let mask = self.call(idx, f, 1, &[ct]).map(|l| l[1]).unwrap_or(0);
            let supported = (0..7u8).filter(|b| mask & (1 << b) != 0).map(ext_rate_hz).collect();
            return Ok(ReportRate { hz: ext_rate_hz(r[0]), supported });
        }
        let f = self.feature_index(idx, 0x8060)?;
        let mask = self.call(idx, f, 0, &[])?[0];
        let r = self.call(idx, f, 1, &[])?;
        let supported = (0..8u8).filter(|b| mask & (1 << b) != 0).map(|b| 1000 / (b as u16 + 1)).collect();
        Ok(ReportRate { hz: if r[0] == 0 { 0 } else { 1000 / r[0] as u16 }, supported })
    }

    pub fn set_report_rate(&self, idx: u8, hz: u16) -> Result<()> {
        if let Ok(f) = self.feature_index(idx, 0x8061) {
            let code = (0..7u8).find(|&c| ext_rate_hz(c) == hz).ok_or(HidppError::V2(0x02))?;
            self.call(idx, f, 3, &[code])?;
            return Ok(());
        }
        let f = self.feature_index(idx, 0x8060)?;
        let ms = (1000 / hz.max(1)).clamp(1, 8) as u8;
        self.call(idx, f, 2, &[ms])?;
        Ok(())
    }

    /// 0x8100 Onboard Profiles mode: 1 = onboard (the mouse runs its stored
    /// profile and may override host DPI), 2 = host. `None` without the feature.
    pub fn onboard_mode(&self, idx: u8) -> Option<u8> {
        let f = self.feature_index(idx, 0x8100).ok()?;
        self.call(idx, f, 2, &[]).ok().map(|r| r[0])
    }

    /// Current onboard DPI level index (0x8100 getCurrentDpiIndex).
    pub fn current_dpi_index(&self, idx: u8) -> Result<u8> {
        let f = self.feature_index(idx, 0x8100)?;
        Ok(self.call(idx, f, 11, &[])?[0])
    }

    /// Switch the active onboard DPI level (0x8100 setCurrentDpiIndex) —
    /// only meaningful while the mouse is in onboard mode.
    pub fn set_current_dpi_index(&self, idx: u8, level: u8) -> Result<()> {
        let f = self.feature_index(idx, 0x8100)?;
        self.call(idx, f, 12, &[level])?;
        Ok(())
    }
}

/// 0x8061 rate code → Hz: 0=8ms 1=4ms 2=2ms 3=1ms 4=500µs 5=250µs 6=125µs.
fn ext_rate_hz(code: u8) -> u16 {
    match code { 0 => 125, 1 => 250, 2 => 500, 3 => 1000, 4 => 2000, 5 => 4000, 6 => 8000, _ => 0 }
}

/// Decode a DPI list stream: values, with `0xE000 | step` between two values
/// meaning "every `step` from the previous value to the next".
fn parse_dpi_list(bytes: &[u8]) -> Vec<DpiSegment> {
    let mut out = Vec::new();
    let mut prev: Option<u16> = None;
    let mut pending_step = 0u16;
    for c in bytes.chunks(2) {
        if c.len() < 2 {
            break;
        }
        let v = u16::from_be_bytes([c[0], c[1]]);
        if v == 0 {
            break;
        }
        if v & 0xe000 == 0xe000 {
            pending_step = v & 0x1fff;
            continue;
        }
        match prev {
            Some(from) if pending_step > 0 => out.push(DpiSegment { from, to: v, step: pending_step }),
            // A bare value with no step before it is a discrete level.
            _ => out.push(DpiSegment { from: v, to: v, step: 1 }),
        }
        pending_step = 0;
        prev = Some(v);
    }
    // Collapse a leading discrete entry that is really the start of a range.
    if out.len() > 1 && out[0].from == out[0].to && out[0].to == out[1].from {
        out.remove(0);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeState {
    Discharging,
    Charging,
    Full,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct Battery {
    pub percent: Option<u8>,
    pub millivolts: Option<u16>,
    pub status: ChargeState,
}

/// `from..=to` every `step` DPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DpiSegment {
    pub from: u16,
    pub to: u16,
    pub step: u16,
}

#[derive(Debug, Clone, Default)]
pub struct Dpi {
    pub current: u16,
    pub default: u16,
    pub segments: Vec<DpiSegment>,
    /// Onboard DPI levels (0x2202 only).
    pub presets: Vec<u16>,
    /// Lift-off distance, passed back unchanged on write.
    pub lod: u8,
}

impl Dpi {
    pub fn min(&self) -> u16 {
        self.segments.first().map_or(0, |s| s.from)
    }
    pub fn max(&self) -> u16 {
        self.segments.last().map_or(0, |s| s.to)
    }
    /// Nearest value the sensor accepts.
    pub fn snap(&self, v: u16) -> u16 {
        if self.segments.is_empty() {
            return v;
        }
        let v = v.clamp(self.min(), self.max());
        for s in &self.segments {
            if v >= s.from && v <= s.to {
                let k = (v - s.from + s.step / 2) / s.step;
                return (s.from + k * s.step).min(s.to);
            }
        }
        // In a gap between segments: pick the closer boundary.
        let below = self.segments.iter().filter(|s| s.to <= v).map(|s| s.to).max().unwrap_or(self.min());
        let above = self.segments.iter().filter(|s| s.from >= v).map(|s| s.from).min().unwrap_or(self.max());
        if v - below <= above - v { below } else { above }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReportRate {
    pub hz: u16,
    pub supported: Vec<u16>,
}

/// A slot in a receiver's pairing table, plus whatever the device answered.
#[derive(Debug, Clone)]
pub struct Device {
    pub index: u8,
    /// Name from the receiver's pairing table (available even when offline).
    pub paired_name: String,
    /// Wireless PID from the pairing table.
    pub wpid: u16,
    pub kind: &'static str,
    pub online: bool,
    pub protocol: Option<(u8, u8)>,
    /// Name reported by the device itself (online only).
    pub name: Option<String>,
    pub battery: Option<Battery>,
    pub dpi: Option<Dpi>,
    pub report_rate: Option<ReportRate>,
    /// 0x8100 onboard-profiles mode (1 onboard, 2 host), if the feature exists.
    pub onboard_mode: Option<u8>,
    pub error: Option<String>,
}

/// How a [`Receiver`] entry is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// A Unifying / Lightspeed / Bolt receiver with a pairing table.
    Receiver,
    /// A device plugged in over USB, talking HID++ itself.
    Usb,
    /// A device paired over Bluetooth (vendor page 0xFF43).
    Bluetooth,
}

/// Direct-device index: the device itself rather than a pairing slot.
pub const DIRECT_INDEX: u8 = 0xff;

pub struct Receiver {
    pub pid: u16,
    pub product: String,
    pub transport: Transport,
    pub link: Link,
}

/// Receiver product ids; anything else on the vendor page is a device.
fn receiver_kind(pid: u16) -> Option<&'static str> {
    Some(match pid {
        0xc52b | 0xc532 | 0xc517 | 0xc51b | 0xc526 | 0xc52e | 0xc531 | 0xc542 => "Unifying receiver",
        0xc52f | 0xc534 | 0xc53d => "Nano receiver",
        0xc539 | 0xc53a | 0xc53f | 0xc547 | 0xc54d | 0xc541 | 0xc545 => "Lightspeed receiver",
        0xc548 => "Bolt receiver",
        _ => return None,
    })
}

impl Receiver {
    pub fn kind(&self) -> &'static str {
        match self.transport {
            Transport::Receiver => receiver_kind(self.pid).unwrap_or("receiver"),
            Transport::Usb => "USB",
            Transport::Bluetooth => "Bluetooth",
        }
    }

    /// Read the pairing table (HID++ 1.0 register 0xB5) and probe each slot;
    /// for a direct device, probe the device itself.
    pub fn devices(&self) -> Vec<Device> {
        let mut out = Vec::new();
        if self.transport != Transport::Receiver {
            let mut d = Device {
                index: DIRECT_INDEX,
                paired_name: self.product.clone(),
                wpid: self.pid,
                kind: "device",
                online: false,
                protocol: None,
                name: None,
                battery: None,
                dpi: None,
                report_rate: None,
                onboard_mode: None,
                error: None,
            };
            self.probe(&mut d);
            if d.online {
                d.kind = self.link.device_type(DIRECT_INDEX).unwrap_or("device");
            }
            out.push(d);
            return out;
        }
        for n in 0..6u8 {
            let Ok(info) = self.link.register(0x83, 0xb5, &[0x20 | n]) else { continue };
            if info.len() < 12 {
                continue;
            }
            let wpid = u16::from_be_bytes([info[7], info[8]]);
            // Reply layout after the 0x20|n echo: destId, interval, wpid(2), ..., type at +7.
            let kind = match info[11] & 0x0f {
                0x01 => "keyboard",
                0x02 => "mouse",
                0x03 => "numpad",
                0x04 => "presenter",
                0x07 => "remote",
                0x08 => "trackball",
                0x09 => "touchpad",
                0x0a => "tablet",
                0x0b => "gamepad",
                0x0c => "joystick",
                _ => "device",
            };
            let paired_name = self
                .link
                .register(0x83, 0xb5, &[0x40 | n])
                .ok()
                .and_then(|r| {
                    let len = (*r.get(5)? as usize).min(14);
                    Some(String::from_utf8_lossy(&r[6..6 + len.min(r.len().saturating_sub(6))]).trim().to_string())
                })
                .unwrap_or_default();
            let mut d = Device {
                index: n + 1,
                paired_name,
                wpid,
                kind,
                online: false,
                protocol: None,
                name: None,
                battery: None,
                dpi: None,
                report_rate: None,
                onboard_mode: None,
                error: None,
            };
            self.probe(&mut d);
            out.push(d);
        }
        out
    }

    /// Refresh the live fields of one device.
    pub fn probe(&self, d: &mut Device) {
        match self.link.ping(d.index) {
            Ok(Some(v)) => {
                d.online = true;
                d.protocol = Some(v);
                d.error = None;
                d.name = self.link.device_name(d.index).ok();
                d.battery = self.link.battery(d.index).ok();
                d.dpi = self.link.dpi(d.index).ok();
                d.report_rate = self.link.report_rate(d.index).ok();
                d.onboard_mode = self.link.onboard_mode(d.index);
            }
            Ok(None) => {
                d.online = false;
                d.protocol = None;
                d.battery = None;
            }
            Err(e) => {
                d.online = false;
                d.error = Some(e.to_string());
            }
        }
    }
}

/// (interface path, pid, product, transport, short collection, long collection)
type Group = (String, u16, String, Transport, Option<HidDevice>, Option<HidDevice>);

/// Open every Logitech receiver and directly attached HID++ device found.
pub fn receivers() -> std::result::Result<Vec<Receiver>, String> {
    let api = HidApi::new().map_err(|e| e.to_string())?;
    // Group the vendor collections by interface path (everything before `&Col`).
    let mut groups: Vec<Group> = Vec::new();
    for info in api.device_list() {
        if info.vendor_id() != LOGITECH {
            continue;
        }
        let transport = match info.usage_page() {
            0xff00 if receiver_kind(info.product_id()).is_some() => Transport::Receiver,
            0xff00 => Transport::Usb,
            0xff43 => Transport::Bluetooth,
            _ => continue,
        };
        let path = info.path().to_string_lossy().into_owned();
        let base = path.split("&Col").next().unwrap_or(&path).to_ascii_lowercase();
        let dev = match info.open_device(&api) {
            Ok(d) => d,
            Err(e) => {
                utility_core::logger::log(format!("hid: cannot open {path}: {e}"));
                continue;
            }
        };
        let entry = match groups.iter_mut().find(|g| g.0 == base) {
            Some(g) => g,
            None => {
                groups.push((base.clone(), info.product_id(), info.product_string().unwrap_or("").to_string(), transport, None, None));
                groups.last_mut().unwrap()
            }
        };
        // Bluetooth carries long reports only (usage 0x0202 on page 0xFF43).
        match (transport, info.usage()) {
            (Transport::Bluetooth, _) => entry.5 = Some(dev),
            (_, 1) => entry.4 = Some(dev),
            _ => entry.5 = Some(dev),
        }
    }
    Ok(groups
        .into_iter()
        .map(|(_, pid, product, transport, short, long)| Receiver { pid, product, transport, link: Link { short, long } })
        .collect())
}

/// Open receivers and return the one with `pid` (first match).
pub fn receiver(pid: u16) -> std::result::Result<Receiver, String> {
    receivers()?.into_iter().find(|r| r.pid == pid).ok_or_else(|| format!("receiver {pid:04x} not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Range stream captured from a G PRO X2 SUPERSTRIKE (0x2202 fn 2, three replies).
    const X2_RANGES: &[u8] = &[
        0x00, 0x64, 0xe0, 0x01, 0x00, 0xc8, 0xe0, 0x02, 0x01, 0xf4, 0xe0, 0x05, 0x03, 0xe8, 0xe0, 0x0a, 0x07, 0xd0, 0xe0, 0x14,
        0x13, 0x88, 0xe0, 0x32, 0x27, 0x10, 0xe0, 0x64, 0x4e, 0x20, 0xe0, 0x7d, 0x7d, 0x00, 0xe0, 0xc8, 0xab, 0xe0, 0x00, 0x00,
    ];

    #[test]
    fn dpi_ranges_decode() {
        let segs = parse_dpi_list(X2_RANGES);
        assert_eq!(segs.len(), 9);
        assert_eq!(segs[0], DpiSegment { from: 100, to: 200, step: 1 });
        assert_eq!(segs[3], DpiSegment { from: 1000, to: 2000, step: 10 });
        assert_eq!(segs[8], DpiSegment { from: 32000, to: 44000, step: 200 });
    }

    #[test]
    fn dpi_snap() {
        let dpi = Dpi { segments: parse_dpi_list(X2_RANGES), ..Default::default() };
        assert_eq!((dpi.min(), dpi.max()), (100, 44000));
        assert_eq!(dpi.snap(1600), 1600);
        assert_eq!(dpi.snap(1604), 1600);
        assert_eq!(dpi.snap(1606), 1610);
        assert_eq!(dpi.snap(5), 100);
        assert_eq!(dpi.snap(60000), 44000);
        assert_eq!(dpi.snap(33333), 33400);
        assert_eq!(dpi.snap(25010), 25000);
    }

    #[test]
    fn dpi_discrete_list() {
        // 0x2201 style: 800, 1600, 3200 with no steps.
        let segs = parse_dpi_list(&[0x03, 0x20, 0x06, 0x40, 0x0c, 0x80, 0, 0]);
        assert_eq!(segs.len(), 3);
        let dpi = Dpi { segments: segs, ..Default::default() };
        assert_eq!(dpi.snap(1000), 800);
        assert_eq!(dpi.snap(1300), 1600);
    }

    #[test]
    fn rate_codes() {
        assert_eq!(ext_rate_hz(3), 1000);
        assert_eq!(ext_rate_hz(6), 8000);
    }

    #[test]
    fn receiver_kinds() {
        let r = Receiver { pid: 0xc54d, product: String::new(), transport: Transport::Receiver, link: Link { short: None, long: None } };
        assert_eq!(r.kind(), "Lightspeed receiver");
        assert!(receiver_kind(0xc548).is_some());
        assert!(receiver_kind(0xc09d).is_none(), "a G PRO mouse pid is a device, not a receiver");
    }
}
