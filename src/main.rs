#[cfg(not(windows))]
compile_error!("mouseutility targets Windows");

mod app;
mod hidpp;
mod ui;

use utility_core::{cli, AppInfo};

pub static APP: AppInfo = AppInfo {
    name: "mouseutility",
    display_name: "Mouse Utility",
    repo: "viorizz/mouseutility",
    tagline: "gaming mice · battery · DPI · HID++",
    version: env!("CARGO_PKG_VERSION"),
    build_epoch: env!("BUILD_EPOCH"),
};

fn main() -> anyhow::Result<()> {
    if let Some(code) = cli::handle_common_args(&APP) {
        std::process::exit(code);
    }
    utility_core::init(&APP);

    if std::env::args().any(|a| a == "--list" || a == "-l") {
        return list();
    }
    if let Some(pos) = std::env::args().position(|a| a == "--call") {
        return call(std::env::args().skip(pos + 1).collect());
    }
    if let Some(pos) = std::env::args().position(|a| a == "--set-dpi" || a == "--set-rate") {
        let args: Vec<String> = std::env::args().collect();
        let value: u16 = args.get(pos + 1).and_then(|v| v.parse().ok()).ok_or_else(|| anyhow::anyhow!("usage: {} <value>", args[pos]))?;
        return set(args[pos] == "--set-dpi", value);
    }

    let cfg: app::Config = utility_core::config::load();

    if std::env::args().any(|a| a == "--snapshot") {
        let mut app = app::App::new(cfg);
        print!("{}", app.snapshot(110, 30, std::time::Duration::from_secs(8))?);
        return Ok(());
    }
    cli::auto_update_on_launch(&APP, cfg.core.auto_update);

    let mut terminal = ratatui::init();
    let _ = utility_core::ui::set_terminal_title(&APP);
    let mut app = app::App::new(cfg);
    let result = app.run(&mut terminal);
    ratatui::restore();
    if app.restart_requested {
        println!("{}: restarting into the new version…", APP.name);
        std::process::exit(utility_core::update::relaunch().map_err(|e| anyhow::anyhow!(e))?);
    }
    result
}

/// `mouseutility --list` — print receivers and paired devices, then exit.
fn list() -> anyhow::Result<()> {
    let receivers = hidpp::receivers().map_err(|e| anyhow::anyhow!(e))?;
    if receivers.is_empty() {
        println!("no Logitech receiver found");
    }
    for r in &receivers {
        println!("{} {:04x} {}", r.kind(), r.pid, r.product);
        for d in r.devices() {
            let state = if d.online { "online" } else { "offline" };
            println!("  slot {} {:<8} {:<20} wpid {:04x}  {state}", d.index, d.kind, d.paired_name, d.wpid);
            if let Some(n) = &d.name {
                println!("         name: {n}");
            }
            if let Some(b) = d.battery {
                println!("         battery: {:?} {:?} {:?}", b.percent, b.millivolts, b.status);
            }
            if let Some(dpi) = d.dpi {
                let default = if dpi.default > 0 { format!(", default {}", dpi.default) } else { String::new() };
                let segs: Vec<String> = dpi.segments.iter().map(|s| format!("{}-{}/{}", s.from, s.to, s.step)).collect();
                println!("         dpi: {} ({}-{}{default})  ranges {}", dpi.current, dpi.min(), dpi.max(), segs.join(" "));
                if !dpi.presets.is_empty() {
                    println!("         dpi levels: {:?}  lod {}", dpi.presets, dpi.lod);
                }
            }
            if let Some(rr) = &d.report_rate {
                println!("         report rate: {} Hz  (supported {:?})", rr.hz, rr.supported);
            }
            if let Some(m) = d.onboard_mode {
                println!("         onboard profiles: {}", match m { 1 => "onboard", 2 => "host", _ => "?" });
            }
            if let Some(e) = &d.error {
                println!("         error: {e}");
            }
        }
    }
    Ok(())
}

/// `--set-dpi <dpi>` / `--set-rate <hz>` on the first online mouse.
fn set(dpi: bool, value: u16) -> anyhow::Result<()> {
    let (r, mut d) = first_online()?;
    let msg = if dpi {
        let cur = d.dpi.clone().ok_or_else(|| anyhow::anyhow!("device does not expose DPI"))?;
        app::apply_dpi(&r, &mut d, &cur, cur.snap(value)).map_err(|e| anyhow::anyhow!(e))?
    } else {
        app::apply_rate(&r, &mut d, value).map_err(|e| anyhow::anyhow!(e))?
    };
    println!("{msg}");
    Ok(())
}

/// The first receiver with an online device, and that device.
fn first_online() -> anyhow::Result<(hidpp::Receiver, hidpp::Device)> {
    for r in hidpp::receivers().map_err(|e| anyhow::anyhow!(e))? {
        if let Some(d) = r.devices().into_iter().find(|d| d.online) {
            return Ok((r, d));
        }
    }
    anyhow::bail!("no online device")
}

/// `mouseutility --call <feature-hex> <fn> [param hex bytes…]` — raw HID++ 2.0
/// call to the first online device, for development. Prints the 16 reply bytes.
fn call(args: Vec<String>) -> anyhow::Result<()> {
    let feature = u16::from_str_radix(args.first().map(|s| s.trim_start_matches("0x")).unwrap_or(""), 16)
        .map_err(|_| anyhow::anyhow!("usage: --call <feature-hex> <fn> [hex bytes…]"))?;
    let func: u8 = args.get(1).and_then(|s| s.parse().ok()).ok_or_else(|| anyhow::anyhow!("missing fn"))?;
    let params: Vec<u8> = args[2..].iter().map(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16)).collect::<Result<_, _>>()?;
    let (r, d) = first_online()?;
    let f = r.link.feature_index(d.index, feature)?;
    let reply = r.link.call(d.index, f, func, &params)?;
    println!("slot {} feature {feature:#06x} at index {f:#04x} fn {func}:", d.index);
    println!("  {}", reply.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
    Ok(())
}
