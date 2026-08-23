# Changelog

Every release needs a `## vX.Y.Z` section here — the release workflow
extracts it as the GitHub release notes and fails without one.

## v0.3.0

### Added
- **Onboard DPI levels modal** (`o`): the levels the DPI button cycles through, current one highlighted; Enter makes the selected level active — via `0x8100` setCurrentDpiIndex when the mouse runs its onboard profile, as a plain DPI write in host mode.
- **Low-battery toast**: when a mouse drops to `low_battery_percent` (config, default 20; 0 disables) while discharging, a Windows toast fires once per session per device (re-armed once it charges back above the threshold) and the status line turns red. Honours the shared `notify` setting.
- **Battery history sparkline** in the details panel, built from the 4 s polls (last ~16 min), with min–max and the toast threshold as a caption.
- Shell-driven tests: the DPI / report-rate modals and the low-battery warning are exercised through the full key stack with an offline shell and rendered frames.
- **Direct-connected devices**: Logitech devices plugged in over USB (vendor page `0xFF00` on a non-receiver product id) or paired over Bluetooth (`0xFF43`, long reports only) are enumerated beside receivers and probed at device index `0xFF`, with the kind from `0x0005` getDeviceType. Read-only and untested on real hardware so far — no such device was available; please report what you see.

### Not done, deliberately
- Rewriting the DPI levels *stored* in the onboard profile: the G PRO X2 SUPERSTRIKE accepts the documented `0x8100` startWrite/writeData sequence but always rejects endWrite with error 0x04 (tried counts 4–0x1000, onboard and host mode, with and without G HUB running). Until that is understood, profile memory stays read-only — the levels shown come from `0x2202`.

### Changed
- Built on **utility-core v0.2.0**: the app loop, update dialog, help overlay, status line and snapshot renderer now come from `utility_core::shell`; MouseUtility only carries its panels, keys and its own modals.
- CI / Release workflows use SHA-pinned actions; `packaging/release.ps1` cuts a release after checking CHANGELOG, Cargo.toml and the tree.

## v0.2.0

Writes arrive: set DPI and report rate from the TUI or the CLI. Verified on a G PRO X2 SUPERSTRIKE over Lightspeed.

### Added
- `d` sets the DPI: type a value (snapped to the nearest step the sensor accepts), or step through the mouse's onboard DPI levels with `←`/`→`. Written with `0x2202` setSensorDpiParameters (or `0x2201` setSensorDpi), then read back — the status line reports old → new and the change is logged.
- `p` sets the report rate from the list the mouse reports as supported (`0x8061` getReportRateList / `0x8060`). Applies to the active (wireless) link.
- Details panel shows the onboard DPI levels (current one highlighted) and the `0x8100` onboard-profiles mode; when a mouse runs an onboard profile the DPI modal warns the mouse may revert the change.
- CLI: `--set-dpi <dpi>`, `--set-rate <hz>` for scripts; `--call <feature-hex> <fn> [hex bytes…]` raw HID++ 2.0 call for debugging.
- Unit tests for the DPI range-list decoder and value snapping, using a captured range stream.

### Fixed
- Report rate on Lightspeed 8K mice: v0.1 asked `0x8061` for connection type 0 (wired) and showed 1000 Hz; the active wireless link (type 1) is now read first, so an 8000 Hz mouse shows 8000 Hz.
- `0x2202` default DPI was read from the wrong offset (always 0); it now comes from the defaultX field.
- DPI ranges are decoded as a list of segments with their own step (e.g. 100–200/1 … 32000–44000/200) instead of a single min/max/step, so snapping is exact across the whole range.
- `--list` no longer prints `default 0`.

### Changed
- Built on utility-core v0.1.1.

## v0.1.0

First release — read-only Logitech HID++ over `hidapi`, on [utility-core](https://github.com/viorizz/utility-core).

- Finds Unifying / Lightspeed / Bolt receivers and reads their pairing table (name, wireless id, device kind) so a sleeping mouse is still listed, marked offline.
- For an online device: name (`0x0005`), battery (`0x1004` / `0x1000` / `0x1001`), DPI with supported range (`0x2202` extended or `0x2201`), report rate (`0x8061` / `0x8060`), HID++ version.
- Device list with battery at a glance, details panel with an ASCII mouse and a colour-coded battery gauge; re-polls every 4 s.
- `--list` non-interactive listing, `--snapshot` renders one frame for layout checks.
- From utility-core: Shift+U SHA-256-verified self-update, `c` copy log, config in `%APPDATA%\mouseutility`.
