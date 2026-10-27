# glide

A low-latency software KVM: share one keyboard and mouse across your computers.
Push the cursor off a screen edge and it continues on the next machine.

- **Fast**: about 10 µs of overhead on top of the network (under 1 ms wired, a few ms on Wi-Fi).
- **Feels native**: same pointer speed on every PC, high-polling-rate mice, scan-code keys for games.
- **Smart switching**: window drags snap instead of switching; optional edge delay; Ctrl+Shift+L locks.
- **One set of speakers**: the other PCs' sound plays on the PC you sit at.
- **Shared clipboard**: copy on one PC, paste on another (text, formatting, pictures).
- **Typing follows your click**: look at or scroll another screen and keep typing where you were.
- **No setup pain**: pair with a 6-digit code over LAN, Wi-Fi or Tailscale; no firewall rules.
- **Updates itself** and launches at sign-in.

Windows and macOS (Apple Silicon), in any mix. Linux is planned.

## Get started

1. From the [latest release](../../releases/latest), on each PC:
   - **Windows**: download `glide-windows.zip`, unzip it into its own folder, and open `glide.exe`.
   - **Mac**: download `glide-macos.zip`, unzip it, and drag `glide.app` into Applications.
     See [Installing on a Mac](docs/USAGE.md#installing-on-a-mac) for the two one-time prompts.
2. On the PC with the keyboard and mouse, choose **This one shares**. It shows an address and a 6-digit code.
3. On each other PC, choose **This one is controlled**, enter that address and code, and press **Pair**.
4. Back on the first PC, drag the new device to the side of your screens it sits on.

## Docs

- [Using glide](docs/USAGE.md): tray, startup, updates, shortcuts, sound, clipboard, settings, command line
- [Troubleshooting](docs/TROUBLESHOOTING.md)
- [Development](docs/DEVELOPMENT.md) and [Architecture](docs/ARCHITECTURE.md)
