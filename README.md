# Mouse Utility

A terminal mouse utility for Windows: see your Logitech receivers and the
mice paired to them — battery, DPI, report rate — from a TUI. Talks Logitech
**HID++** directly through `hidapi`; no G HUB, no driver, read-only.

Part of the *Utility line, built on [utility-core](https://github.com/viorizz/utility-core).

```
╭ Devices ─────────────────────────────╮╭ Details ───────────────────────────────────────────────────────────╮
│ ⌁ Lightspeed receiver  c54d          ││   ╭────┬────╮      PRO X2 SUPERSTRIKE                              │
│   ▸ ● PRO X2 SUPERSTRIKE 88%         ││  ╱     │     ╲     ● online                                        │
│                                      ││ │      ╵      │                                                    │
│                                      ││ │   ╭──┴──╮   │       receiver  Lightspeed receiver (c54d) · slot 1│
│                                      ││ │   │     │   │           kind  mouse                              │
│                                      ││ │   ╰─────╯   │    wireless id  40bd                               │
│                                      ││ │             │       protocol  HID++ 4.2                          │
│                                      ││  ╲           ╱             dpi  1600  (range 100–44000)            │
│                                      ││   ╰─────────╯      report rate  1000 Hz                            │
```

## Install

```powershell
irm https://raw.githubusercontent.com/viorizz/mouseutility/main/install.ps1 | iex
```

## What it reads

| | HID++ feature |
| --- | --- |
| receiver pairing table (name, wireless id, kind — even when the mouse is asleep) | 1.0 register `0xB5` |
| device name | `0x0005` |
| battery | `0x1004` unified, `0x1000` status, `0x1001` voltage |
| DPI and supported range | `0x2202` extended, `0x2201` adjustable |
| report rate | `0x8061` extended, `0x8060` |

A paired mouse that is asleep or switched off shows as **offline**; the panel
re-polls every few seconds and picks it up when it wakes. Unifying,
Lightspeed and Bolt receivers are recognised.

Nothing is written to the mouse in this version.

## Keys

| Key | Action |
| --- | --- |
| `↑` `↓` | select a device |
| `r` | rescan |
| `Shift+U` | check for / install an update |
| `c` | copy the session log to the clipboard |
| `?` / `q` | help / quit |

CLI: `mouseutility --list` prints receivers and devices and exits;
`--version`, `--update`, `--no-update-check` as in every *Utility tool.

## Build

```powershell
cargo build --release
```

## License

MIT
