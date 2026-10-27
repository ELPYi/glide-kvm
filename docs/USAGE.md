# Using glide

## Installing on a Mac

glide isn't from the App Store, so macOS asks twice, once each:

1. **Opening it**: the first time, macOS says it can't check glide. Open
   System Settings, Privacy & Security, scroll down and click **Open Anyway**
   next to glide. (Or, in Terminal: `xattr -dr com.apple.quarantine /Applications/glide.app`.)
2. **Accessibility**: when sharing first starts, macOS asks to let glide
   control the computer (it needs this to move the cursor and type, and to
   block the Mac's own keyboard while you're on another PC). Turn glide on in
   System Settings, Privacy & Security, Accessibility, then quit glide (its
   menu bar icon, right-click, Quit) and open it again.

Updates keep both: the app is signed the same way every time.

On a Mac, the Windows key is Command and Alt is Option, in the same places
on the keyboard. Settings are kept in `~/Library/Application Support/glide`.
Sound sharing isn't available on a Mac yet.

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

**Settings → Updates** shows the version, has **Check now** (and
**Install now** once an update is downloaded, to restart into it straight
away), and can turn automatic updates off. Copies you build yourself are "dev" builds and never
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
| **Typing goes to the PC I last clicked on** | On by default. Move the cursor to another screen to look or scroll and your typing stays where you were; click on a PC to type there. Held Ctrl / Shift / Alt / Win move with the click, so Ctrl+click works anywhere. Off: the keyboard goes wherever the cursor goes. |

### Gaming: stay on this PC

While a game is in focus, the screen edges don't switch PCs, so flicking the
mouse to the edge mid-game never throws it onto another PC. A game is an app
that fills its whole screen (fullscreen or borderless) or traps the mouse in
its window. This works on whichever PC the game runs on.

Fullscreen video never counts: browsers (YouTube, Netflix, Twitch...) and
video players (VLC, mpv, Media Player...) keep switching normally. The header
shows **In game: staying here** while it's active.

**Settings → Gaming** turns it off, or lists programs that should always or
never count (e.g. a windowed game, or a fullscreen app that isn't a game).

### Only one cursor at a time

While you're on another PC, this PC's cursor is hidden, and it reappears
where you come back in. The other PC hides its cursor when you leave it, and
shows it again the moment its own mouse or touchpad moves. (Apps that draw
their own custom cursor, such as some games, may still show theirs.)

## Sound from the other PCs

Everything the other PCs play (videos, music, calls, notifications) comes
out of the sharing PC's speakers or headset, mixed with its own sound. It
streams all the time, whichever PC you're on, about 70-80 ms behind (most of
that is a small buffer that keeps Wi-Fi hiccups from crackling). That's
unnoticeable for video, music and calls; you might notice it in rhythm games
played on the other PC.

While a PC is sending its sound, its own speakers are muted so you don't hear
it twice. They're unmuted when glide stops or loses the sharing PC.

The switches apply right away, no Save needed:

- **On the sharing PC, Settings → Sound**: turn it off, or set how loud the
  other PCs are (this PC's own volume is unchanged). The Devices tab shows
  **Sound** next to a PC whose sound is playing.
- **One PC only, on the sharing PC's Devices tab**: **Mute** silences that
  PC's sound (it stays silent: its own speakers stay muted too); **Unmute**
  brings it back. Remembered across restarts.
- **On another PC, Settings → Sound**: stop sending its sound, or keep its
  speakers on while sending.

It uses about 16 KB/s of network and a fraction of a percent of CPU, on its
own threads, so the mouse is unaffected. Silence sends nothing. Both PCs need
glide 0.4.7 or later.

## Copy and paste between PCs

Copy on any PC and paste on any other: text, formatted text (from browsers
and Office) and pictures (screenshots, copied images). Whatever you copy is
sent to the other PCs in the background, a fraction of a second for a
screenshot, through glide's own encrypted connection.

- Passwords copied from a password manager are never sent (they mark them
  private, and glide respects that).
- Files copied in Explorer aren't shared.
- **Settings → Clipboard** turns it off on a PC; its copies then stay on it,
  and it ignores the others'.

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
