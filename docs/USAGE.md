# Using glide

## Running in the background

Closing the window (X) keeps glide running in the **tray**, the icons by the
clock (click the **^** arrow if it's hidden there):

- **Left-click** the icon to open glide.
- **Right-click** it for **Lock / unlock switching** and **Quit glide**.
- Opening `glide.exe` again while it's running brings the window back
  instead of starting a second copy.

Prefer the X to quit? Turn off **Settings → Startup → Keep running in the tray**.

## One-click from then on

- glide **launches into the tray when you sign in** (turned on the first time a
  downloaded copy runs; change it in **Settings → Startup → When I sign in**).
- **Settings → Startup → Start sharing automatically**: opening glide starts it.
- Pin `glide.exe` to the taskbar or Start menu for a one-click launch.

## Updates

Downloaded copies keep themselves up to date. Every push to `master` builds
and publishes a release.
glide checks for one shortly after starting and every 30 minutes, downloads it
in the background and verifies its checksum. It installs the next time you
leave the PC alone for a minute, and never while you're controlling another PC.
glide restarts itself and shows **glide updated to …** on screen.

**Settings → Updates** shows the version, has **Check now**, and can turn
automatic updates off. Copies you build yourself are "dev" builds and never
update themselves.

## Shortcuts and switching

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

### Only one cursor at a time

While you're on another PC, this PC's cursor is hidden, and it reappears
where you come back in. The other PC hides its cursor when you leave it, and
shows it again the moment its own mouse or touchpad moves. (Apps that draw
their own custom cursor, such as some games, may still show theirs.)

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
it's plain text; see [glide.example.toml](../glide.example.toml).
