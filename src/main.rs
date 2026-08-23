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
                let default = if dpi.default > 0 { format!("default {}, ", dpi.default) } else { String::new() };
                println!("         dpi: {} ({default}{}-{} step {})", dpi.current, dpi.min, dpi.max, dpi.step);
            }
            if let Some(hz) = d.report_rate {
                println!("         report rate: {hz} Hz");
            }
            if let Some(e) = &d.error {
                println!("         error: {e}");
            }
        }
    }
    Ok(())
}
