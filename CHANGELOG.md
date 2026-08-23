# Changelog

Every release needs a `## vX.Y.Z` section here — the release workflow
extracts it as the GitHub release notes and fails without one.

## v0.1.0

First release — read-only Logitech HID++ over `hidapi`, on [utility-core](https://github.com/viorizz/utility-core).

- Finds Unifying / Lightspeed / Bolt receivers and reads their pairing table (name, wireless id, device kind) so a sleeping mouse is still listed, marked offline.
- For an online device: name (`0x0005`), battery (`0x1004` / `0x1000` / `0x1001`), DPI with supported range (`0x2202` extended or `0x2201`), report rate (`0x8061` / `0x8060`), HID++ version.
- Device list with battery at a glance, details panel with an ASCII mouse and a colour-coded battery gauge; re-polls every 4 s.
- `--list` non-interactive listing, `--snapshot` renders one frame for layout checks.
- From utility-core: Shift+U SHA-256-verified self-update, `c` copy log, config in `%APPDATA%\mouseutility`.
