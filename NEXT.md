# Next round — MouseUtility toward MVP (then winget)

1. **Onboard profiles / DPI levels** (feature 0x8100): show the levels the DPI button cycles
   through, let the user pick/edit the active one. Extend the 0x2202 preset work.
2. **Direct-connected Logitech devices**: Bluetooth (usage page 0xFF43, report id 0x11,
   device index 0xFF) and wired USB (0xFF00 on a non-receiver PID), listed beside receivers.
3. **Low-battery toast** via `utility_core::notify` (threshold in config, once per session)
   and a battery-history sparkline in the details panel from the 4 s polls.
4. When utility-core tags **v0.2.0** (shared `shell` module), bump and delete the local copies
   of the update modal / event loop.
5. Writes keep the confirm-before-apply pattern; anything not testable on the G PRO X2
   SUPERSTRIKE stays read-only. clippy -D warnings + tests green, CHANGELOG `## v0.3.0`,
   commit to main; tagging coordinated by the DiskUtility session; winget at MVP.
