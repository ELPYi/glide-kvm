# glide

A low-latency software KVM: share one keyboard and mouse across your computers.
Push the cursor off a screen edge and it continues on the next machine.

- **Fast**: about 10 µs of overhead on top of the network (under 1 ms wired, a few ms on Wi-Fi).
- **Feels native**: same pointer speed on every PC, high-polling-rate mice, scan-code keys for games.
- **Smart switching**: window drags snap instead of switching; optional edge delay; Ctrl+Shift+L locks.
- **No setup pain**: pair with a 6-digit code over LAN, Wi-Fi or Tailscale; no firewall rules.
- **Updates itself** and launches at sign-in.

Windows today; Linux and macOS are planned.

## Get started

1. Download `glide-windows.zip` from the [latest release](../../releases/latest), unzip it into
   its own folder on each PC, and open `glide.exe`.
2. On the PC with the keyboard and mouse, choose **This one shares**. It shows an address and a 6-digit code.
3. On each other PC, choose **This one is controlled**, enter that address and code, and press **Pair**.
4. Back on the first PC, drag the new device to the side of your screens it sits on.

## Docs

- [Using glide](docs/USAGE.md): tray, startup, updates, shortcuts, settings, command line
- [Troubleshooting](docs/TROUBLESHOOTING.md)
- [Development](docs/DEVELOPMENT.md) and [Architecture](docs/ARCHITECTURE.md)
