use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, List, ListItem, ListState, Paragraph, Sparkline, Wrap};
use ratatui::Frame;
use utility_core::shell::Shell;
use utility_core::ui::{modal_block, Hint, Theme};

use crate::app::{App, Modal};
use crate::hidpp::{ChargeState, Device};

pub const THEME: Theme = Theme::MOUSE;

pub const HINTS: &[Hint] = &[("↑↓", "select"), ("d", "dpi"), ("p", "rate"), ("o", "levels"), ("r", "rescan"), ("U", "update"), ("c", "log"), ("?", "help"), ("q", "quit")];

/// A small mouse, top view.
const MOUSE_ART: [&str; 9] = [
    "   ╭────┬────╮   ",
    "  ╱     │     ╲  ",
    " │      ╵      │ ",
    " │   ╭──┴──╮   │ ",
    " │   │     │   │ ",
    " │   ╰─────╯   │ ",
    " │             │ ",
    "  ╲           ╱  ",
    "   ╰─────────╯   ",
];

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let [head, body, foot] = Shell::layout(area);

    let mut status = app.shell.header_status();
    status.show_elevation = false;
    app.shell.draw_header(f, head, &status);

    let cols = Layout::horizontal([Constraint::Length(40), Constraint::Min(30)]).split(body);
    draw_devices(f, cols[0], app);
    draw_details(f, cols[1], app);

    app.shell.draw_footer(f, foot, HINTS);

    match &app.modal {
        Modal::None => {}
        Modal::Dpi { input } => draw_dpi(f, area, app, input),
        Modal::Rate { sel } => draw_rate(f, area, app, *sel),
        Modal::Levels { sel, input } => draw_levels(f, area, app, *sel, input),
    }
}

fn draw_devices(f: &mut Frame, area: Rect, app: &App) {
    let mut title = vec![Span::styled(" Devices ", Style::new().fg(THEME.text).bold())];
    if app.scanning {
        title.push(Span::styled(format!("{} ", app.shell.spinner()), Style::new().fg(THEME.accent)));
    }
    let block = THEME.bordered(Line::from(title));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(e) = &app.scan_error {
        f.render_widget(Paragraph::new(Span::styled(format!(" {e}"), Style::new().fg(THEME.err))).wrap(Wrap { trim: false }), inner);
        return;
    }
    if app.receivers.is_empty() {
        let msg = if app.last_scan.is_none() { " scanning…" } else { " No Logitech receiver found." };
        f.render_widget(Paragraph::new(THEME.dim(msg)), inner);
        return;
    }

    let mut items: Vec<ListItem> = Vec::new();
    let mut flat = 0usize;
    let mut sel_row = 0usize;
    for r in &app.receivers {
        items.push(ListItem::new(Line::from(vec![
            Span::styled(" ⌁ ", Style::new().fg(THEME.accent)),
            Span::styled(r.kind, Style::new().fg(THEME.text).bold()),
            THEME.dim(format!("  {:04x}", r.pid)),
        ])));
        if r.devices.is_empty() {
            items.push(ListItem::new(Line::from(THEME.dim("     nothing paired"))));
        }
        for d in &r.devices {
            let selected = flat == app.sel;
            if selected {
                sel_row = items.len();
            }
            let (dot, style) = if d.online { ("●", Style::new().fg(THEME.ok)) } else { ("○", Style::new().fg(THEME.dim)) };
            let name = d.name.clone().unwrap_or_else(|| d.paired_name.clone());
            let name = if name.is_empty() { format!("{} {:04x}", d.kind, d.wpid) } else { name };
            let mut line = Line::from(vec![
                Span::styled(if selected { "   ▸ " } else { "     " }, Style::new().fg(THEME.accent)),
                Span::styled(dot, style),
                Span::raw(" "),
                Span::styled(
                    format!("{:<18}", utility_core::text::fit(&name, 18)),
                    if selected { Style::new().fg(THEME.text).bold() } else { Style::new().fg(THEME.text) },
                ),
                THEME.dim(battery_short(d)),
            ]);
            if selected {
                line = line.style(Style::new().bg(THEME.sel_bg));
            }
            items.push(ListItem::new(line));
            flat += 1;
        }
    }
    let mut state = ListState::default().with_selected(Some(sel_row));
    f.render_stateful_widget(List::new(items), inner, &mut state);
}

fn battery_short(d: &Device) -> String {
    match d.battery {
        Some(b) => match (b.percent, b.millivolts) {
            (Some(p), _) => format!("{p:>3}%"),
            (None, Some(mv)) => format!("{:.2}V", mv as f32 / 1000.0),
            _ => String::new(),
        },
        None if d.online => String::new(),
        None => "off".into(),
    }
}

fn draw_details(f: &mut Frame, area: Rect, app: &App) {
    let block = THEME.bordered(Line::from(Span::styled(" Details ", Style::new().fg(THEME.text).bold())));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some((ri, d)) = app.selected() else {
        f.render_widget(Paragraph::new(THEME.dim("  Select a device.")), inner);
        return;
    };
    let r = &app.receivers[ri];

    let cols = Layout::horizontal([Constraint::Length(19), Constraint::Min(20)]).split(inner);
    let art: Vec<Line> = MOUSE_ART
        .iter()
        .map(|l| Line::from(Span::styled(*l, Style::new().fg(if d.online { THEME.accent } else { THEME.dim }))))
        .collect();
    f.render_widget(Paragraph::new(art), cols[0]);

    let label = |k: &str| THEME.dim(format!("{k:>12}  "));
    let val = |v: String| Span::styled(v, Style::new().fg(THEME.text));
    let name = d.name.clone().unwrap_or_else(|| d.paired_name.clone());
    let mut lines = vec![
        Line::from(vec![Span::styled(format!(" {name}"), Style::new().fg(THEME.text).bold())]),
        Line::from(vec![
            Span::raw(" "),
            if d.online {
                Span::styled("● online", Style::new().fg(THEME.ok))
            } else {
                Span::styled("○ offline — asleep or switched off", Style::new().fg(THEME.warn))
            },
        ]),
        Line::from(""),
        Line::from(vec![
            label(if d.index == crate::hidpp::DIRECT_INDEX { "link" } else { "receiver" }),
            val(if d.index == crate::hidpp::DIRECT_INDEX {
                format!("{} ({:04x})", r.kind, r.pid)
            } else {
                format!("{} ({:04x}) · slot {}", r.kind, r.pid, d.index)
            }),
        ]),
        Line::from(vec![label("kind"), val(d.kind.to_string())]),
        Line::from(vec![label("wireless id"), val(format!("{:04x}", d.wpid))]),
    ];
    if let Some((maj, min)) = d.protocol {
        lines.push(Line::from(vec![label("protocol"), val(format!("HID++ {maj}.{min}"))]));
    }
    if let Some(dpi) = &d.dpi {
        let mut notes = Vec::new();
        if dpi.max() > 0 {
            notes.push(format!("{}–{}", dpi.min(), dpi.max()));
        }
        if dpi.default > 0 {
            notes.push(format!("default {}", dpi.default));
        }
        let notes = if notes.is_empty() { String::new() } else { format!("  ({})", notes.join(", ")) };
        lines.push(Line::from(vec![label("dpi"), val(dpi.current.to_string()), THEME.dim(notes)]));
        if !dpi.presets.is_empty() {
            let levels: Vec<Span> = dpi
                .presets
                .iter()
                .map(|&p| {
                    let s = format!(" {p} ");
                    if p == dpi.current { Span::styled(s, Style::new().fg(THEME.accent).bold()) } else { THEME.dim(s) }
                })
                .collect();
            let mut row = vec![label("levels")];
            row.extend(levels);
            lines.push(Line::from(row));
        }
    }
    if let Some(rr) = &d.report_rate {
        lines.push(Line::from(vec![label("report rate"), val(format!("{} Hz", rr.hz))]));
    }
    if let Some(m) = d.onboard_mode {
        let txt = match m { 1 => "onboard — the mouse runs its stored profile", 2 => "host", _ => "?" };
        lines.push(Line::from(vec![label("profiles"), val(txt.to_string())]));
    }
    if let Some(e) = &d.error {
        lines.push(Line::from(Span::styled(format!("  {e}"), Style::new().fg(THEME.err))));
    }
    if !d.online {
        lines.push(Line::from(""));
        lines.push(Line::from(THEME.dim("  Move the mouse to wake it; the panel refreshes every few seconds.")));
    }
    let text_h = lines.len() as u16;
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), cols[1]);

    if let Some(b) = d.battery {
        let gauge_area = Rect { x: cols[1].x + 1, y: cols[1].y + text_h + 1, width: cols[1].width.saturating_sub(2).min(40), height: 1 };
        if gauge_area.y < inner.y + inner.height {
            let status = match b.status {
                ChargeState::Charging => " charging",
                ChargeState::Full => " full",
                ChargeState::Discharging => "",
                ChargeState::Unknown => "",
            };
            let (ratio, text) = match (b.percent, b.millivolts) {
                (Some(p), _) => (p as f64 / 100.0, format!("battery {p}%{status}")),
                (None, Some(mv)) => (((mv as f64 - 3500.0) / 700.0).clamp(0.0, 1.0), format!("battery {:.2} V{status}", mv as f64 / 1000.0)),
                _ => (0.0, "battery".into()),
            };
            let color = if ratio < 0.15 { THEME.err } else if ratio < 0.35 { THEME.warn } else { THEME.ok };
            f.render_widget(
                Gauge::default().ratio(ratio).label(text).gauge_style(Style::new().fg(color).bg(THEME.gauge_bg)),
                gauge_area,
            );
            // Battery history from the periodic polls, newest on the right.
            let key = crate::app::device_key(r.pid, d);
            if let Some(h) = app.history.get(&key).filter(|h| h.len() >= 2) {
                let spark = Rect { x: gauge_area.x, y: gauge_area.y + 2, width: gauge_area.width, height: 2 };
                if spark.y + spark.height <= inner.y + inner.height {
                    let w = spark.width as usize;
                    let data: Vec<u64> = h.iter().skip(h.len().saturating_sub(w)).map(|&p| p as u64).collect();
                    let (lo, hi) = (h.iter().min().copied().unwrap_or(0), h.iter().max().copied().unwrap_or(0));
                    f.render_widget(Sparkline::default().data(&data).max(100).style(Style::new().fg(color)), spark);
                    let cap = Rect { x: spark.x, y: spark.y + spark.height, width: spark.width, height: 1 };
                    if cap.y < inner.y + inner.height {
                        let mins = h.len() * 4 / 60;
                        f.render_widget(
                            Paragraph::new(THEME.dim(format!("last {mins} min · {lo}–{hi}%  (toast at {}%)", app.cfg.low_battery_percent))),
                            cap,
                        );
                    }
                }
            }
        }
    }
}

fn draw_dpi(f: &mut Frame, area: Rect, app: &App, input: &str) {
    let inner = modal_block(f, area, 60, 11, "Set DPI", THEME.accent);
    let Some((_, d)) = app.selected() else { return };
    let Some(dpi) = &d.dpi else { return };
    let typed: Option<u16> = input.parse().ok();
    let snapped = typed.map(|v| dpi.snap(v));
    let name = d.name.clone().unwrap_or_else(|| d.paired_name.clone());
    let mut lines = vec![
        Line::from(vec![THEME.dim("  "), Span::styled(name, Style::new().fg(THEME.text).bold()), THEME.dim(format!("  now {} dpi", dpi.current))]),
        Line::from(""),
        Line::from(vec![
            THEME.dim("  new dpi  "),
            Span::styled(format!("{input}▏"), Style::new().fg(THEME.text).bold()),
            match (typed, snapped) {
                (Some(t), Some(s)) if s != t => Span::styled(format!("  → {s} (nearest supported)"), Style::new().fg(THEME.warn)),
                (None, _) if !input.is_empty() => Span::styled("  not a number", Style::new().fg(THEME.err)),
                _ => Span::raw(""),
            },
        ]),
        Line::from(THEME.dim(format!("  range {}–{}", dpi.min(), dpi.max()))),
    ];
    if !dpi.presets.is_empty() {
        let levels = dpi.presets.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("  ");
        lines.push(Line::from(THEME.dim(format!("  onboard levels  {levels}   (←→ to step)"))));
    }
    if d.onboard_mode == Some(1) {
        lines.push(Line::from(Span::styled("  onboard profile active — the mouse may revert on its own", Style::new().fg(THEME.warn))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![THEME.key("  Enter"), THEME.dim(" apply   "), THEME.key("Esc"), THEME.dim(" cancel")]));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_rate(f: &mut Frame, area: Rect, app: &App, sel: usize) {
    let inner = modal_block(f, area, 60, 9, "Set report rate", THEME.accent);
    let Some((_, d)) = app.selected() else { return };
    let Some(rr) = &d.report_rate else { return };
    let name = d.name.clone().unwrap_or_else(|| d.paired_name.clone());
    let mut row = vec![THEME.dim("  ")];
    for (i, hz) in rr.supported.iter().enumerate() {
        let s = format!(" {hz} ");
        row.push(if i == sel {
            Span::styled(s, Style::new().fg(THEME.text).bg(THEME.sel_bg).bold())
        } else if *hz == rr.hz {
            Span::styled(s, Style::new().fg(THEME.accent))
        } else {
            THEME.dim(s)
        });
        row.push(Span::raw(" "));
    }
    let lines = vec![
        Line::from(vec![THEME.dim("  "), Span::styled(name, Style::new().fg(THEME.text).bold()), THEME.dim(format!("  now {} Hz", rr.hz))]),
        Line::from(""),
        Line::from(row),
        Line::from(""),
        Line::from(THEME.dim("  Higher rates cost battery; applies to the active link.")),
        Line::from(vec![THEME.key("  Enter"), THEME.dim(" apply   "), THEME.key("←→"), THEME.dim(" choose   "), THEME.key("Esc"), THEME.dim(" cancel")]),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_levels(f: &mut Frame, area: Rect, app: &App, sel: usize, input: &str) {
    let inner = modal_block(f, area, 62, 12, "Onboard DPI levels", THEME.accent);
    let Some((_, d)) = app.selected() else { return };
    let Some(dpi) = &d.dpi else { return };
    let name = d.name.clone().unwrap_or_else(|| d.paired_name.clone());
    let mut row = vec![THEME.dim("  ")];
    for (i, p) in dpi.presets.iter().enumerate() {
        let s = format!(" {p} ");
        row.push(if i == sel {
            Span::styled(s, Style::new().fg(THEME.text).bg(THEME.sel_bg).bold())
        } else if *p == dpi.current {
            Span::styled(s, Style::new().fg(THEME.accent))
        } else {
            THEME.dim(s)
        });
        row.push(Span::raw(" "));
    }
    let typed: Option<u16> = input.parse().ok();
    let snapped = typed.map(|v| dpi.snap(v));
    let mut lines = vec![
        Line::from(vec![THEME.dim("  "), Span::styled(name, Style::new().fg(THEME.text).bold()), THEME.dim(format!("  now {} dpi", dpi.current))]),
        Line::from(""),
        Line::from(row),
        Line::from(""),
    ];
    if input.is_empty() {
        lines.push(Line::from(THEME.dim("  Enter makes this level the active DPI. Type a value to")));
        lines.push(Line::from(THEME.dim("  rewrite the level in the mouse's stored profile instead.")));
    } else {
        lines.push(Line::from(vec![
            THEME.dim(format!("  rewrite level {} to  ", sel + 1)),
            Span::styled(format!("{input}▏"), Style::new().fg(THEME.text).bold()),
            match (typed, snapped) {
                (Some(t), Some(sn)) if sn != t => Span::styled(format!("  → {sn} (nearest supported)"), Style::new().fg(THEME.warn)),
                (None, _) => Span::styled("  not a number", Style::new().fg(THEME.err)),
                _ => Span::raw(""),
            },
        ]));
        lines.push(Line::from(THEME.dim("  This edits the profile stored on the mouse itself.")));
    }
    if d.onboard_mode == Some(2) {
        lines.push(Line::from(THEME.dim("  Profiles are in host mode — a picked level is set as plain DPI.")));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        THEME.key("  Enter"),
        THEME.dim(" apply   "),
        THEME.key("←→"),
        THEME.dim(" level   "),
        THEME.key("Esc"),
        THEME.dim(" cancel"),
    ]));
    f.render_widget(Paragraph::new(lines), inner);
}
