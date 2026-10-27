# glide architecture

How glide is put together, why, and how to change it safely. If you're new
to Rust, read the [primer](#rust-primer-for-this-codebase) first.

## The big picture

```
 ┌──────────── server (has keyboard + mouse) ─────────────┐        ┌────────── client ──────────┐
 │                                                        │        │                            │
 │  OS hooks ──► platform::capture ──► Server::on_captured│  UDP   │ client loop ──► Desktop    │
 │  (keys,         (own thread)          │                │ ─────► │  decrypt,       ::inject   │
 │   buttons,                            ▼                │ encrypted│ reorder,      (SendInput)│
 │   raw motion)                  switcher::Switcher      │ ◄───── │  ack                       │
 │                              (pure decisions)          │ acks,  │                            │
 │                                       │                │ pongs, │                            │
 │                       net::protocol + net::crypto      │ edge-  │                            │
 │                                                        │ leaves │                            │
 └────────────────────────────────────────────────────────┘        └────────────────────────────┘
                 ▲                                                            ▲
                 └──────── engine::Engine (start / stop / status) ────────────┘
                                         ▲
                       gui (desktop app)  │  bin/glide-cli (headless)
```

The UI and CLI never touch sockets or hooks. They call `Engine`, which wires
config, platform and networking together.

## Module map

| Path | What it does | Depends on |
|---|---|---|
| `src/bin/glide.rs` | App entry: opens the window | `gui` |
| `src/bin/glide-cli.rs` | CLI entry: `server` / `client` / `genkey` / `selftest` | `engine` |
| `src/engine.rs` | Start/stop/status facade | everything below |
| `src/config.rs` | `glide.toml` load / save / validate | `input` |
| `src/server/mod.rs` | Server I/O: sockets, threads, applying decisions | `switcher`, `peer`, `net`, `platform` |
| `src/server/switcher.rs` | **Pure** switching rules: edges, drag-block, delay, lock, hotkeys | `input` |
| `src/server/pacer.rs` | **Pure** motion pacing: merges mouse reports, ≤1 packet/ms | — |
| `src/server/peer.rs` | Per-client state: address, liveness, RTT, outbox | `net` |
| `src/client/mod.rs` | Client loop: receive, drain + merge, inject, knock, mirror pointer feel | `net`, `platform` |
| `src/net/protocol.rs` | Message types and their byte layout | `input` |
| `src/net/crypto.rs` | Encryption, authentication, replay protection | — |
| `src/net/reliable.rs` | **Pure** ordered/acked delivery (`Outbox`, `Inbox`) | — |
| `src/net/addr.rs` | Resolving typed addresses; listing this PC's own (LAN / Tailscale) | — |
| `src/net/pairing.rs` | 6-digit-code pairing over SPAKE2 (server state machine + client function) | — |
| `src/input.rs` | OS-neutral events (keys are Linux evdev codes) | — |
| `src/hotkey.rs` | **Pure** shortcuts: parse/display "Ctrl+Shift+L", exact matching, key names | — |
| `src/status.rs` | Read-only snapshots for display | — |
| `src/audio/mod.rs` | Sound from the other PCs: settings, device traits (`AudioIn`, `AudioOut`), `Uplink` | — |
| `src/audio/codec.rs` | Opus encoder/decoder (10 ms frames, 128 kbps, low-delay mode) | `unsafe-libopus` |
| `src/audio/playout.rs` | **Pure** jitter buffer: adaptive depth, loss concealment, clock-drift nudging | `codec` |
| `src/audio/share.rs` | Client side: capture, cut into frames, encode, send; quiet during silence | `codec`, `Uplink` |
| `src/audio/play.rs` | Server side: per-PC playout buffers, mixing, opening the speakers only while sound arrives | `playout` |
| `src/clip/mod.rs` | Shared clipboard: `ClipData` (text, HTML, DIB picture) and its deflated wire form | `miniz_oxide` |
| `src/clip/transfer.rs` | **Pure** chunked transfer over UDP: window, cumulative acks, resends only the holes | — |
| `src/clip/sync.rs` | Clipboard worker: local copies out to every PC, arrivals onto this one (the server relays) | `transfer` |
| `src/update.rs` | Over-the-air updates from GitHub Releases: version compare, checksum, stage/swap files, background checker | `platform` (`http_get`) |
| `src/logging.rs` | `log!` macro + in-memory log for the Logs tab | `platform` (clock) |
| `src/platform/mod.rs` | The OS contract: `Desktop` trait, `start_capture`, `system` | — |
| `src/platform/geometry.rs` | **Pure** monitor layout: outer-edge detection, entry points (multi-monitor, L-shaped) | `input` |
| `src/platform/windows/` | Windows implementation, one file per concern: `capture`, `desktop`, `keymap`, `pointer` (speed/acceleration), `timing` (sub-ms sleep), `qos` (network priority), `http` (HTTPS via WinHTTP, for updates), `audio` (WASAPI loopback + playback), `system`, `selftest` | Win32 |
| `src/platform/{linux,macos}.rs` | Plans + stubs (`unsupported.rs`) | — |
| `src/gui/` | The app; see [The app](#the-app-gui) | `engine`, `config`, `status` |

Dependencies point **down**: `gui` → `engine` → `server`/`client` → `net`/`platform` → `input`.
Nothing low-level knows about the UI. Keep it that way. If a lower module
seems to need something from a higher one, pass it in as a parameter instead.

## Design decisions

**Pure logic is separate from I/O.** `Switcher` decides; `Server` does.
`Outbox`/`Inbox` track sequence numbers; the server and client move bytes.
Pure parts take time as a parameter (`now: Instant`) instead of reading a
clock, so tests can say "200 ms later" without sleeping. When you add a
rule, put it in a pure module and test it there.

**One trait for the OS.** Everything platform-specific is behind
`platform::Desktop` plus a few functions. Tests swap in a `MockDesktop` that
records what would have been injected.

**Latency first.**
- Captured events are encrypted and sent *inside the OS hook callback*, with
  no queue or thread hop. Mouse motion is fire-and-forget UDP. Keys, buttons
  and wheel use a tiny ack/retransmit layer, so a lost packet can't leave a
  key stuck.
- Measured end-to-end in software: ~10 µs at p50 (`cargo test latency -- --nocapture`).
- Mouse motion comes from Raw Input (unaccelerated); the receiving PC applies
  its own pointer acceleration, so the feel matches its local mouse.

**Staying snappy under load.** Two things made the cursor lag on the
controlled PC, worst while dragging a window:
1. *A packet backlog.* A gaming mouse reports up to 8,000×/s. During a window
   drag Windows processes each injected move slowly, so replaying packets
   one by one fell further and further behind. Now the server merges motion
   and sends at most one packet per millisecond (`server/pacer.rs`; the first
   move after a pause goes out immediately). On each wake-up the client
   drains *every* waiting packet and injects their sum once. The
   `tests/snappy.rs` tests simulate a 3 ms-per-inject PC: the client catches
   up within ~4 ms instead of falling ~1 s behind.
2. *Double pointer ballistics.* Injected relative moves get the receiving
   PC's acceleration ("Enhance pointer precision") on top of the raw
   counts. So `Enter` carries the sharing PC's pointer speed and
   acceleration, the client adopts them while active (not saved, restored on
   leave), and the mouse feels identical everywhere. You can turn this off.

**Where the latency goes, and what glide does about each part.** Measured
on this project's PCs (both on Wi-Fi):

| Stage | Typical | What glide does |
|---|---|---|
| Hook → encrypt → send (server) | ~5 µs | Sent inline in the hook; no queues or thread hops |
| Motion pacing | 0–0.25 ms (wired) / 0–1 ms (Wi-Fi) | First move after a pause goes at once; interval adapts to the link's RTT |
| Network, one way | ~0.1 ms wired, ~2–3 ms Wi-Fi | Packets tagged *voice* priority (qWAVE → WMM); keep-warm heartbeats every 25 ms stop radios dozing (dozing adds 10–100 ms to the next packet) |
| Receive → inject (client) | ~20 µs while moving | Sleeps in `recv` until a packet arrives, then drains and merges whatever else is waiting so it never falls behind. (It used to spin on the socket to save ~20 µs per packet; that burned a whole core while the mouse moved, for a gain lost in the network's milliseconds.) |
| Waiting for a CPU on a busy PC | was 4–6 ms p50 / 25 ms p99 at full load → now ~0.1 ms / 1.3 ms | Hot threads are registered with MMCSS ("Pro Audio") and the process runs at high priority (`platform/windows/sched.rs`; `cargo test --release --test snappy under_cpu_load -- --ignored --nocapture`) |
| Display | up to 1 frame | Outside glide's control (higher refresh rate helps) |

Software overhead end to end: p50 ~20 µs while moving, ~27 µs for the first
move after a pause (`cargo test --test snappy latency -- --nocapture`). The
network is everything else, which is why the Devices tab shows each link's
typical round trip *and* its spikes. On Wi-Fi, a cable on either PC is
worth more than any code change.

**Tray and single instance.** Hidden windows don't run egui frames, so the
tray's menu can't go through `App`. Its handlers act directly: they show
the window via the OS, or lock/quit through the shared `EngineSlot`.
`bin/glide.rs` claims a named mutex. A second launch finds the running
window, shows it, and exits. A copy started by the admin task waits up to
5 s for the mutex, because the copy that launched it may still be closing.

**Administrator mode.** Windows' UIPI stops a normal program from injecting
into, or hooking input aimed at, admin windows (Task Manager). With admin
mode on, one UAC prompt registers a "highest privileges" scheduled task
(`platform/windows/admin.rs`). `bin/glide.rs` then hands every normal launch
off to that task (no prompt) and exits. The task runs at normal priority with
no time limit, and can also start glide at sign-in.

**Keeping glide light.** Measured on this project's PC (window open, sharing
on, no devices):

| | Before | After | What changed |
|---|---|---|---|
| Private memory | 444 MB | ~113 MB | OpenGL renderer (`glow`) instead of wgpu; no system fonts |
| Program size | 12 MB | 6.8 MB | same |
| CPU, using the PC normally | ~3.7% of a core | ~0.5–1% | capture: monitor layout cached and cursor taken from hook data (was ~6 OS calls per mouse report); motion skipped entirely while local |
| CPU, idle in the tray | ~3% | ~0.2% | timer sleeps until the next job instead of waking 200×/s |
| CPU, using the PC normally, 8 kHz mouse | ~3.7% of a core (capture thread) | ~0.4% | the mouse hook is installed only while controlling another PC; locally Raw Input gives the buttons and `GetCursorPos` the edge position. Windows calls a mouse hook synchronously on every report before moving the cursor, so this also takes glide off your own cursor's path. `glide-cli selftest` checks the switch-over on the real OS |
| CPU, controlling another PC with an 8 kHz mouse | ~180% of a core (both ends) | ~3.5% | client sleeps in `recv` instead of spinning; pacer sleeps on a high-resolution timer instead of spinning the last 0.3 ms (`cargo test --release --test cpu -- --ignored --nocapture`) |

Measure with the scripts' approach: per-thread CPU via thread names
(`glide-capture`, `glide-srv-*`, `glide-client`). The `GLIDE_INSTANCE`
environment variable lets a test copy run beside your real glide.

Built-in fonts only: `gui/font_check.rs` fails the build's tests if any UI
text uses a character those fonts can't draw.

**Shortcuts give feedback.** Shortcut changes apply and save instantly
(`Action::SetHotkey`). Using the lock shortcut shows an on-screen
confirmation (`Desktop::notify`; on Windows a click-through, non-activating
layered window in `platform/windows/osd.rs`). It shows on the PC you're
using: locally, or via `ReliableEv::Notice` on the active client.

End-to-end GUI tests: `GLIDE_TEST_INJECTED=1` makes a test copy's capture react
to synthetic (SendInput) keys/mouse instead of the physical devices, so
a script can click "Change", press a combination, and check the result.

**No limbo.** Windows silently drops injected input on the secure desktop
(lock screen, UAC, Ctrl+Alt+Del), and drops it under UIPI when an admin
window is in front of a non-admin glide. The server would then keep "controlling"
a PC that ignores it, so neither PC responds. The client checks ~10×/s
(`platform/windows/blocking.rs`: input desktop name, foreground process
elevation) and reports it in every `Pong`. The server won't cross into a
blocked PC and leaves immediately if the active one becomes blocked
(`tests/limbo.rs`). Network loss is the other limbo; both sides now give up
after 1 s without traffic.

**Shortcuts survive missed key releases.** Keys whose release goes
elsewhere (Ctrl+Shift+Esc opening admin Task Manager, Win+L, Ctrl+Alt+Del)
would look held forever and break exact shortcut matching. Before each key
press while local, capture reports the OS's real modifier state
(`Captured::Modifiers`), and the switcher drops stale ones.

**Rules apply in both directions.** Drag protection and edge delay hold
whether you're leaving the sharing PC or coming back from a client. The
switcher tracks mouse buttons held *on the client* (the clicks it
forwarded), so a window drag there snaps instead of returning.

**Shortcuts are data.** `hotkey::Hotkeys` lives in config, the switcher
matches them, and Settings records new ones by asking the engine to
capture the next combination. That goes through the same low-level hook,
so keys the GUI toolkit can't see (Scroll Lock, Pause) work too.

**No stuck keys, ever.** The client tracks what it has pressed and releases
all of it on leave, on timeout (1.5 s) and on stop. The server's switcher
remembers where each held key's press went (`key_dest`) and sends its
repeats and release to the same PC, whatever has switched since.

**Keyboard follows the last click** (`keyboard_follows_click`, default on).
The mouse goes where the cursor is; new key presses go to the PC last clicked
on (`Switcher::keyboard`). A click moves the keyboard to the cursor's PC, and
held modifiers move with it (released where they were pressed, pressed on
the new PC, before the click), so Ctrl+click works anywhere. Clients inject
keys even while the cursor is elsewhere. If that PC disconnects or locks,
typing returns to the server (`keyboard_lost`).

### Updates

- **Where they come from.** CI (`.github/workflows/release.yml`) builds every push to `master` and
  publishes a GitHub release: `glide.exe`, `glide-cli.exe`, and `latest.toml` (version + SHA-256 of
  each). `build.rs` bakes `GLIDE_VERSION` and `GLIDE_UPDATE_REPO` in; a build without them is a
  "dev" build (`is_release_build()` is false) and never touches updates or sign-in settings.
- **No TLS library.** Downloads use Windows' own WinHTTP (`platform::http_get`): system
  certificates and proxy settings, nothing extra to compile or keep patched.
- **Swap, don't overwrite.** A running exe can't be deleted on Windows, but it can be renamed.
  The verified download is written as `glide.exe.new`; installing renames `glide.exe` →
  `glide.exe.old` and `.new` → `glide.exe`, then starts the new copy with `--after-update` and
  exits. The new copy waits for the single-instance lock, deletes `*.old`, and shows an OSD.
- **Only when idle.** Installing waits until there's been no input for a minute and this PC isn't
  controlling another, so an update never interrupts you mid-use.

### Sound

- **Capture**: Windows' per-app capture (process loopback, Windows 10 2004+) records every
  app's sound except glide's own, as 48 kHz stereo float. It taps the apps' streams before they
  reach any speakers, so the client can mute its own speakers and still send everything. The
  older endpoint loopback (fallback) taps the speakers' mix, which on most PCs is also before the
  mute, but some drivers' sound effects apply the mute first and it then records silence.
- **Transport**: `ClientMsg::Audio{stream, seq, opus}`, one 10 ms Opus frame per packet, over
  the existing connection (a second socket would need a firewall rule on the server). The
  server asks for sound with a byte appended to `Heartbeat`; older builds don't send it, which
  reads as "no", so mixed versions keep working.
- **Kept off the mouse's path**: audio runs on its own normal-priority threads
  (`glide-audio-share`, `glide-audio-play`). On the server, `glide-srv-recv` decrypts an audio
  packet and hands it to the player through a bounded channel *before* taking the state lock
  the input hook uses. It flows client → server, the opposite direction to mouse packets.
- **Playout** (`audio/playout.rs`): holds ~30 ms before playing, grows by 10 ms after a real
  dropout (not when the sound simply ended), shrinks back after 30 s of steady play. A lost
  packet is concealed by Opus; late ones are dropped. Clock drift between the two sound cards
  is corrected by playing 0.2% faster or slower (linear interpolation) whenever the level
  leaves a ±3 ms band around where it settled.
- **Power**: the server opens its speakers only while sound is arriving and closes them 3 s
  after it stops, so glide doesn't hold the audio device (or keep the PC awake) for nothing.
  The client records only while connected and asked for sound, in event mode: its thread
  wakes once per 10 ms chunk while sound plays and not at all during silence.
- **Latency budget** (≈70-80 ms): capture chunk 10 ms, frame 10 ms, network a few ms, playout
  ~30 ms, speaker queue ~15 ms, plus Windows' own engine buffering.

## Threads

| Thread | Where | Job |
|---|---|---|
| `glide-capture` | `platform/windows/capture.rs` | Hooks + Raw Input message loop (time-critical priority) |
| `glide-srv-pacer` | `server/mod.rs` | Sends merged motion when its 1 ms slot comes up |
| `glide-srv-recv` | `server/mod.rs` | Pongs, acks, knocks, edge-leaves, pairing |
| `glide-srv-timer` | `server/mod.rs` | Heartbeats (25 ms connected / 200 ms searching), retransmits (30 ms), liveness, network priority, pairing upkeep |
| `glide-srv-dns` | `server/mod.rs` | Resolving client addresses (can block) |
| `glide-client` | `client/mod.rs` | Everything on the client (time-critical priority) |
| `glide-audio-share` | `audio/share.rs` | Client: record, encode and send this PC's sound (normal priority) |
| `glide-audio-play` | `audio/play.rs` | Server: decode, buffer, mix and play the other PCs' sound (normal priority) |
| `glide-clipboard` | `clip/sync.rs` | Both: sends and receives clipboards, paced (sleeps between copies) |
| `glide-clipboard-os` | `platform/windows/clipboard.rs` | Both: clipboard listener window; reads copies, writes arrivals |
| main | `gui` | The window; reads status 4×/s |

Server state sits behind one `Mutex`. It's only ever held for microseconds
and never across DNS or sleeps. Every loop checks a `running` flag and uses
100 ms socket timeouts, so `stop()` returns quickly and frees the port.

## Protocol

All UDP, one message per packet, port 24801 by default.

```
packet = [magic 'G'][dir][session u64][counter u64]  ++  ChaCha20-Poly1305(message)
          └───────────── 18-byte header, authenticated ─────────────┘
```

- **Key**: `SHA-256("glide-v1" ‖ secret ‖ session)`. Each server run picks a
  new `session` (its start time in µs), so keys rotate per run.
- **Replay protection**: a 64-packet sliding window over `counter`.
- **Server → client**: `Heartbeat{sent_micros, wants_sound}`, `Motion{dx,dy}` (unreliable),
  `Reliable{seq, Enter{side, frac, feel} | Leave | Input}`, `Clip`.
- **Client → server**: `Pong{instance, echo_micros}` (gives RTT),
  `Ack{seq}` (cumulative), `EdgeLeave{side, frac}`, `Knock`, `Audio{stream, seq, opus}`, `Clip`.
- **Clipboard** (`Clip`, both ways): `Chunk{id, index, total, data}` (≤1100 bytes of the
  deflated clipboard) and `Ack{id, next}` (every chunk below `next` arrived). Paced at ~4 chunks
  per ms so mouse packets never queue behind a burst. Older builds ignore the unknown tag.
- **Knock / firewall**: a client with `server_address` set sends `Knock`
  (sealed with session 0) every 500 ms until connected. Its firewall then
  treats the server's packets as replies, so **no inbound rule is needed**.
  The server answers a valid knock immediately.
- **Restarts**: the client's `instance` id changes per run; the server resets
  that peer's sequence numbers when it sees a new one.

Changing a message layout is a protocol break. Bump the label (currently
`glide-v2`) in `net/crypto.rs` so mismatched versions simply don't talk.

### Pairing

A separate packet family (first byte `'P'`), handled before any session
exists. The sharing PC shows a 6-digit code; the new PC runs SPAKE2 with it:

```
new PC                                   sharing PC
  Hello  {spake_a, name}       ───►      SPAKE2 -> K
                               ◄───      Reply {spake_b, AEAD_K(port, secret)}
  SPAKE2 -> K, decrypt (wrong code = fails here)
  Confirm{AEAD_K(name)}        ───►      device added (first free side)
                               ◄───      Ack   {AEAD_K("ok")}
```

SPAKE2 means an eavesdropper learns nothing, and someone guessing the code
gets one live try per attempt (3 per code, 5-minute lifetime). They can't
test codes offline. Each PC keeps its own port; the new PC records the sharing
PC's port in `server_address` only if they differ. The optional "poke" sends
packets from the sharing PC to the new PC so that a strict firewall on the
sharing PC lets the first hello in.

## The app (GUI)

egui is **immediate mode**: each frame, `App::ui` rebuilds the whole UI
from current state with plain function calls. There are no widget objects
or callbacks to keep in sync.

```
gui/
  app.rs        the only stateful component: config, engine, draft, applies Actions
  actions.rs    every user intent as an enum (Start, SaveSettings, SetLocked, ...)
  theme.rs      palette (by semantic Tone, not raw colors), spacing, icon
  tray.rs       tray icon (Windows/macOS); the X hides here, Quit lives here
  engine_slot.rs  the running engine, shared by the window and the tray
  views/        one file per screen: header, devices, settings, logs, welcome
  widgets/      reusable pieces: card, pill, form rows, layout map
```

The rule is that **views draw and emit `Action`s, and only `app.rs` acts on
them.** Views get a read-only `Model` (plus the settings draft to edit).
So to add a button:

1. add a variant to `Action`,
2. draw the button in a view and `actions.push(...)` it,
3. handle it in `App::apply`.

Settings are edited as a *draft* and applied on Save. `needs_restart()`
decides whether the change can go live (switching options) or needs an
engine restart (port, secret, devices...).

## Testing

```
cargo test                  # everything (~1 s)
cargo test switcher         # just the switching rules
cargo test --test snappy -- --nocapture   # prints merge + catch-up numbers
cargo test latency -- --nocapture   # prints the latency numbers
cargo clippy --all-targets  # lints; keep it at zero warnings
cargo fmt                   # formatting
glide-cli selftest          # real OS hooks on this machine (freezes mouse ~1 s)
```

| Layer | Where | Covers |
|---|---|---|
| Unit | `#[cfg(test)] mod tests` at the bottom of each file | switching rules, motion pacing, reliability, crypto, protocol, pairing, keymap, config, restart logic, sub-ms timer |
| Headless UI | `gui/widgets/layout_map.rs` tests | the real drag-and-drop widget driven with simulated pointer events |
| Integration | `tests/end_to_end.rs` | real server + client over UDP with mock desktops: crossing, latency, drag-block, lock, hotkeys, restarts, wrong secret |
| | `tests/snappy.rs` | 8 kHz mouse merged without loss; slow receiver doesn't lag; pointer feel mirrored + restored; live layout changes |
| | `tests/pairing.rs` | pair via code through a running server, then connect; wrong code; refusal when full |
| | `tests/firewall_knock.rs` | knock answered at once; forged knock ignored; ports released |
| On-device | `glide-cli selftest` | edge detection on your monitors; hooks block input while raw motion still arrives |

The GUI is checked by hand (it's a thin layer); its logic, `needs_restart`
and first-run config creation, is unit-tested in `gui/app.rs`.

## Adding an OS

Implement the contract in `src/platform/mod.rs` in `src/platform/<os>.rs`
(the plans are already written in `linux.rs` and `macos.rs`):

1. `desktop()`: inject keys (map evdev codes, the wire format, to native ones), relative motion, buttons, wheel; warp the cursor; detect edges.
2. `start_capture()`: capture physical input on a thread, call the handler, swallow while grabbed, report edge pushes.
3. `system`: autostart at sign-in, firewall helper, local time.
4. `selftest()`.

Nothing outside `platform/` should need to change.
`cargo check --target x86_64-unknown-linux-gnu` / `aarch64-apple-darwin` keeps the stubs honest.

## Rust primer for this codebase

- **Crate**: a package. This one is a *library* (`src/lib.rs`, all the logic)
  plus two *binaries* in `src/bin/` that are thin wrappers around it.
- **Modules**: `mod foo;` in a file pulls in `foo.rs` or `foo/mod.rs`.
  `pub` makes something visible outside its module, and things are private by
  default. The folder tree *is* the module tree.
- **`Result<T>` and `?`**: functions that can fail return `Result`. `?` means
  "if this failed, return the error now". `anyhow` gives readable error chains
  (`.context("doing X")`).
- **`Option<T>`**: a value that might be absent (`Some(x)` / `None`). Rust has
  no null.
- **Ownership**: each value has one owner. `&x` borrows read-only; `&mut x`
  borrows exclusively. `Arc` = shared ownership across threads. `Mutex` =
  one thread at a time. `AtomicBool` = a lock-free flag.
- **Traits**: like interfaces. `Desktop` is one; `MockDesktop` and
  `WinDesktop` both implement it.
- **`#[cfg(windows)]`**: compile this only on Windows. This is how one
  codebase serves every OS.
- **`#[derive(Debug, Clone, PartialEq)]`**: auto-generate printing, copying
  and `==` for a type.
- **Tests**: any function marked `#[test]` runs under `cargo test`. Unit tests
  sit at the bottom of the file they test; integration tests live in `tests/`.
- **`unsafe`**: only around Win32 calls in `platform/windows/`. That's where
  the compiler can't check for us, so it's kept small and contained.

## Building on Windows

Needs Rust (`rustup`) and, for the GNU toolchain used here, MinGW on `PATH`
(WinLibs: `winget install BrechtSanders.WinLibs.POSIX.UCRT`). The MSVC
toolchain with Visual Studio Build Tools works too.

```
cargo build --release          # target/release/glide.exe + glide-cli.exe
scripts/package.ps1            # dist/glide/: ready-to-copy folder
```
