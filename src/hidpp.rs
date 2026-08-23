//! Logitech HID++ over `hidapi`, read-only.
//!
//! A receiver (Unifying / Lightspeed / Bolt) exposes two vendor collections on
//! usage page 0xFF00: usage 1 carries short reports (0x10, 7 bytes), usage 2
//! long reports (0x11, 20 bytes). On Windows each collection is its own
//! handle, and a reply always arrives on the collection matching *its own*
//! length — so a short request can get a long answer on the other handle.
//! [`Link`] owns both and reads whichever answers.
//!
//! The receiver itself (device index 0xFF) speaks HID++ 1.0 registers; paired
//! devices (index 1..=6) speak HID++ 2.0 features. A paired device that is
//! asleep or switched off answers the ping with HID++ 1.0 error 0x08
//! ("unknown device") — reported here as `online = false`, not as a failure.

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
        let mut min = 0;
        let mut max = 0;
        let mut step = 0;
        let mut i = 1;
        let mut last = 0u16;
        while i + 1 < 16 {
            let v = u16::from_be_bytes([l[i], l[i + 1]]);
            if v == 0 {
                break;
            }
            if v & 0xe000 == 0xe000 {
                step = v & 0x1fff;
            } else {
                if min == 0 {
                    min = v;
                }
                last = v;
            }
            i += 2;
        }
        if last > 0 {
            max = last;
        }
        Ok(Dpi { current, default, min, max, step })
    }

    /// 0x2202 Extended Adjustable DPI: getSensorDpi (fn 5) → sensor, dpiX, dpiY,
    /// lod; getSensorDpiRanges (fn 2: sensor, direction, index) → list of
    /// u16 where 0xE000|n encodes a step between the surrounding values.
    fn dpi_extended(&self, idx: u8, f: u8) -> Result<Dpi> {
        let r = self.call(idx, f, 5, &[0])?;
        let current = u16::from_be_bytes([r[1], r[2]]);
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
        let mut min = 0;
        let mut max = 0;
        let mut step = 0;
        for c in stream.chunks(2) {
            if c.len() < 2 {
                break;
            }
            let v = u16::from_be_bytes([c[0], c[1]]);
            if v == 0 {
                break;
            }
            if v & 0xe000 == 0xe000 {
                step = v & 0x1fff;
            } else {
                if min == 0 {
                    min = v;
                }
                max = max.max(v);
            }
        }
        Ok(Dpi { current, default: 0, min, max, step })
    }

    /// Report rate in Hz (0x8060 legacy or 0x8061 extended).
    pub fn report_rate(&self, idx: u8) -> Result<u16> {
        if let Ok(f) = self.feature_index(idx, 0x8061) {
            let r = self.call(idx, f, 2, &[])?;
            // extended: 0=8ms 1=4ms 2=2ms 3=1ms 4=500us 5=250us 6=125us
            let hz = match r[0] { 0 => 125, 1 => 250, 2 => 500, 3 => 1000, 4 => 2000, 5 => 4000, 6 => 8000, _ => 0 };
            return Ok(hz);
        }
        let f = self.feature_index(idx, 0x8060)?;
        let r = self.call(idx, f, 1, &[])?;
        Ok(if r[0] == 0 { 0 } else { 1000 / r[0] as u16 })
    }
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

#[derive(Debug, Clone, Copy)]
pub struct Dpi {
    pub current: u16,
    pub default: u16,
    pub min: u16,
    pub max: u16,
    pub step: u16,
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
    pub report_rate: Option<u16>,
    pub error: Option<String>,
}

pub struct Receiver {
    pub pid: u16,
    pub product: String,
    pub link: Link,
}

impl Receiver {
    pub fn kind(&self) -> &'static str {
        match self.pid {
            0xc52b | 0xc532 => "Unifying receiver",
            0xc52f | 0xc534 => "Nano receiver",
            0xc539 | 0xc53a | 0xc53f | 0xc547 | 0xc54d | 0xc541 | 0xc545 => "Lightspeed receiver",
            0xc548 => "Bolt receiver",
            _ => "receiver",
        }
    }

    /// Read the pairing table (HID++ 1.0 register 0xB5) and probe each slot.
    pub fn devices(&self) -> Vec<Device> {
        let mut out = Vec::new();
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

/// (interface path, pid, product, short collection, long collection)
type Group = (String, u16, String, Option<HidDevice>, Option<HidDevice>);

/// Open every Logitech receiver found (both vendor collections).
pub fn receivers() -> std::result::Result<Vec<Receiver>, String> {
    let api = HidApi::new().map_err(|e| e.to_string())?;
    // Group the 0xFF00 collections by USB interface path (everything before `&Col`).
    let mut groups: Vec<Group> = Vec::new();
    for info in api.device_list() {
        if info.vendor_id() != LOGITECH || info.usage_page() != 0xff00 {
            continue;
        }
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
                groups.push((base.clone(), info.product_id(), info.product_string().unwrap_or("").to_string(), None, None));
                groups.last_mut().unwrap()
            }
        };
        match info.usage() {
            1 => entry.3 = Some(dev),
            _ => entry.4 = Some(dev),
        }
    }
    Ok(groups
        .into_iter()
        .map(|(_, pid, product, short, long)| Receiver { pid, product, link: Link { short, long } })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiver_kinds() {
        let r = Receiver { pid: 0xc54d, product: String::new(), link: Link { short: None, long: None } };
        assert_eq!(r.kind(), "Lightspeed receiver");
    }
}
