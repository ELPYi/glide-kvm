# glide

A low-latency software KVM: share one keyboard and mouse across your computers.
Push the cursor off a screen edge and it continues on the next machine.

- **Fast**: about 10 µs of software overhead from input to the other PC.
  Network time is on top of that: under 1 ms on wired LAN, a few ms on Wi-Fi.
- **Feels native**:
  - Every PC uses the sharing PC's pointer speed and acceleration, so the
    mouse feels the same everywhere.
  - High-polling-rate mice are paced at 1,000 updates a second, and the
    receiving PC never builds a backlog, even mid window-drag.
  - Keys go by scan code, so games work.
- **Smart switching**:
  - Dragging a window to an edge snaps it instead of switching PCs.
  - An optional edge delay prevents accidental switches.
  - A lock keeps you on one PC.
- **Just works on your network**: wired, Wi-Fi or Tailscale. No firewall
  rules are needed when the client knows the server's address.
- **Safe**: encrypted and authenticated with a shared secret. Keys never get
  stuck, even if the network drops.

Windows today. Linux and macOS are planned; the code is structured for them
(see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)).

## Quick start

1. Download `glide-windows.zip` from the
   [latest release](../../releases/latest), unzip it into a folder of its
   own on each PC (for example `Documents\glide`), and open `glide.exe`.
2. **On the PC with the keyboard and mouse**: open glide and choose
   **This one shares**. It shows this PC's address and a 6-digit code.
3. **On each other PC**: open glide and choose **This one is controlled**.
   Type the address and code from step 2, then press **Pair**.
4. Back on the first PC, **drag** the new device to the side of your screens
   it sits on.

That's it. The Devices tab shows who's connected and the round-trip latency.
The pairing code is single-use and expires after 5 minutes. It securely
transfers the long shared secret, so you never copy it by hand.

### Running in the background

Closing the window (X) keeps glide running in the **tray**, the icons by the
clock (click the **^** arrow if it's hidden there):

- **Left-click** the icon to open glide.
- **Right-click** it for **Lock / unlock switching** and **Quit glide**.
- Opening `glide.exe` again while it's running brings the window back
  instead of starting a second copy.

Prefer the X to quit? Turn off **Settings → Startup → Keep running in the tray**.

### One-click from then on

- glide **launches into the tray when you sign in** (turned on the first time a
  downloaded copy runs; change it in **Settings → Startup → When I sign in**).
- **Settings → Startup → Start sharing automatically**: opening glide starts it.
- Pin `glide.exe` to the taskbar or Start menu for a one-click launch.

### Updates

Downloaded copies keep themselves up to date. Every push to `master` builds
and publishes a release (see [.github/workflows/release.yml](.github/workflows/release.yml)).
glide checks for one shortly after starting and every 3 hours, downloads it
in the background and verifies its checksum. It installs the next time you
leave the PC alone for a minute, and never while you're controlling another PC.
glide restarts itself and shows **glide updated to …** on screen.

**Settings → Updates** shows the version, has **Check now**, and can turn
automatic updates off. Copies you build yourself are "dev" builds and never
update themselves.

## Controls

| On the sharing PC's keyboard | Does |
|---|---|
| **Ctrl + Shift + L** | Lock or unlock switching. While locked you stay on the current PC (good for games). Also in the tray icon's menu. |

Toggling the lock shows a brief **"Switching locked / unlocked"** pop-up on
the screen you're using, even while you're on the other PC or glide is in
the tray.

To change a shortcut, go to **Settings → Shortcuts**, click **Change**, and
press the combination you want. It applies right away. Any key works, with any of Ctrl / Alt / Shift / Win. Shortcuts match exactly, so Ctrl+L won't
fire while you're pressing Ctrl+Shift+L. You can also reset a shortcut or turn it off.

| Setting (Settings → Switching) | Effect |
|---|---|
| **Don't switch PCs while a mouse button is held** | On by default. Dragging a window to an edge snaps it instead of switching, on whichever PC you're using. |
| **Edge delay** | How long you must push against an edge before switching, in either direction (0–500 ms). |
| **Same mouse feel on every PC** | On by default. The PC you're controlling temporarily uses this PC's pointer speed and acceleration, so the cursor feels identical. |

## Troubleshooting

- **The cursor seems stuck and neither PC responds**: glide detects the
  usual causes, won't cross into a PC in these states, and takes control
  back within a fraction of a second if one happens while you're on it:
  - the other PC's lock screen, UAC prompt or Ctrl+Alt+Del screen is
    showing (Windows lets no program control those);
  - an admin window (Task Manager, an installer) is in front there and glide
    isn't running as administrator on it.

  The Devices tab shows "Locked" or "Admin window" when that's the case.

- **A PC shows "Offline"**: check that glide is open there. If it was set
  up by hand, pair it again.
- **Pairing says "no answer"**: open the pairing panel's **Not connecting?**
  section on the sharing PC and enter the other PC's address there.
- **The mouse lags on Wi-Fi**: the Devices tab shows each link's typical
  latency and its spikes, and that's the network itself (glide's own code
  takes ~0.01 ms). A cable on either PC makes the biggest difference. After
  that: 5 GHz Wi-Fi, being close to the router, and on a laptop, Windows'
  power plan → Wireless Adapter Settings → *Maximum Performance*.
- **Still can't connect**: use **Settings → Advanced → Allow glide through
  the firewall** on the PC that can't be reached. It asks for admin.
- **Can't control Task Manager or other admin windows**: turn on **Settings →
  Startup → Run glide as administrator** on that PC (and on the sharing PC,
  so its shortcuts keep working while an admin window is focused). It asks
  for permission once, then launches with admin rights without asking again.
  The UAC prompt itself, the lock screen and Ctrl+Alt+Del can't be
  controlled remotely; Windows keeps those on a protected desktop.
- **Ctrl+Alt+Del** always goes to the PC you're physically at; Windows reserves it.
- The **Logs** tab shows what's happening, and **Copy all** grabs it for a bug report.

## Command line

`glide-cli.exe` runs without a window, for scripts or servers:

```
glide-cli server [-c glide.toml]
glide-cli client [-c glide.toml]
glide-cli genkey       # random secret
glide-cli selftest     # check input capture on this PC (freezes mouse ~1 s)
```

## Config file

Settings live in `glide.toml` next to `glide.exe`. The app writes it, but
it's plain text; see [glide.example.toml](glide.example.toml).

## Development

```
cargo test             # 70+ tests: unit, headless UI, end-to-end over real UDP
cargo clippy --all-targets
cargo build --release  # a "dev" build: never updates itself
scripts/package.ps1    # builds dist/glide/
```

Releases are built by CI, which sets `GLIDE_VERSION` and `GLIDE_UPDATE_REPO`
(read by `build.rs`). Only builds with both set update themselves.

[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains the module layout, threads, protocol and testing
strategy, and includes a short Rust primer.
