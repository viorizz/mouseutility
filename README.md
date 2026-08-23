# Mouse Utility

A terminal mouse utility for Windows: see your Logitech receivers and the
mice paired to them — battery, DPI, report rate — and set DPI and report rate,
from a TUI. Talks Logitech **HID++** directly through `hidapi`; no G HUB, no
driver.

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
│                                      ││  ╲           ╱             dpi  1600  (100–44000, default 800)     │
│                                      ││   ╰─────────╯           levels   800  1200  1600  2400  3200       │
│                                      ││                    report rate  8000 Hz                            │
│                                      ││                       profiles  host                               │
```

## Install

```powershell
irm https://raw.githubusercontent.com/viorizz/mouseutility/main/install.ps1 | iex
```

## What it reads and writes

| | HID++ feature |
| --- | --- |
| receiver pairing table (name, wireless id, kind — even when the mouse is asleep) | 1.0 register `0xB5` |
| device name | `0x0005` |
| battery | `0x1004` unified, `0x1000` status, `0x1001` voltage |
| DPI, supported ranges, onboard levels — **read and set** | `0x2202` extended, `0x2201` adjustable |
| report rate, supported list — **read and set** | `0x8061` extended (active link), `0x8060` |
| onboard-profiles mode | `0x8100` |

A paired mouse that is asleep or switched off shows as **offline**; the panel
re-polls every few seconds and picks it up when it wakes. Unifying,
Lightspeed and Bolt receivers are recognised.

Setting DPI or report rate writes straight to the mouse, reads the value
back, and logs the change. Values you type are snapped to the nearest step the
sensor accepts. If the mouse is running an onboard profile (`0x8100` mode 1)
it may revert a host-set DPI on its own — the DPI dialog says so.

## Keys

| Key | Action |
| --- | --- |
| `↑` `↓` | select a device |
| `d` | set DPI — type a value, `←` `→` step through onboard levels |
| `p` | set report rate — pick from what the mouse supports |
| `r` | rescan |
| `Shift+U` | check for / install an update |
| `c` | copy the session log to the clipboard |
| `?` / `q` | help / quit |

CLI: `mouseutility --list` prints receivers and devices and exits;
`--set-dpi <dpi>` / `--set-rate <hz>` change the first online mouse;
`--call <feature-hex> <fn> [hex bytes…]` sends a raw HID++ 2.0 call (debugging);
`--version`, `--update`, `--no-update-check` as in every *Utility tool.

## Build

```powershell
cargo build --release
```

## License

MIT
