# Troubleshooting

- **The cursor seems stuck and neither PC responds**: glide detects the
  usual causes, won't cross into a PC in these states, and takes control
  back within a fraction of a second if one happens while you're on it:
  - the other PC's lock screen, UAC prompt or Ctrl+Alt+Del screen is
    showing (Windows keeps those out of reach unless **Control this PC's
    sign-in screen** is on there, see below), or a Mac's lock screen;
  - an admin window (Task Manager, an installer) is in front there and glide
    isn't running as administrator on it;
  - the two PCs disagree about which one has the mouse (one stopped hearing
    the other for a moment, on Wi-Fi say). Each tells the other which it
    thinks, so either one notices and the mouse comes back to the PC you're at.

  The Devices tab shows "Locked" or "Admin window" when that's the case. If
  it still happens, open the Logs tab on both PCs right afterwards: the
  lines from that moment say what each one saw.

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
  The UAC prompt itself and the lock screen need the next setting.
- **Can't type the PIN on the other PC's lock screen, or click its UAC
  prompts**: on that PC, turn on **Settings → Startup → Control this PC's
  sign-in screen from another PC** (it needs *Run glide as administrator*
  on). It adds a small Windows service, "glide sign-in screen", which
  turning the setting off removes. Ctrl+Alt+Del still can't be sent (it
  isn't needed for the Windows 11 lock screen: click or press any key). If
  it doesn't work, that PC's Logs tab says why ("sign-in screen: ..."), and
  `glide-cli signin-check`, run as administrator there, tests the link.
  Signing in after a restart, before glide has started, isn't covered yet.
- **Ctrl+Alt+Del** always goes to the PC you're physically at; Windows reserves it.
- **Mac: sharing won't start, or the cursor doesn't move there**: glide needs
  the Accessibility permission. In System Settings, Privacy & Security,
  Accessibility, turn glide on (if it's already on, remove it with the minus
  button and add it again), then quit and reopen glide.
- **No sound from another PC**: check **Settings → Sound** on both PCs, and
  that both run glide 0.4.7 or later. The Logs tab on each says whether it's
  sending or playing, or why it can't (no speakers found, say).
- **Sound crackles**: usually Wi-Fi dropouts. glide keeps a little more in
  hand after each one, so it should settle within seconds. A cable helps
  most.
- **The other PC's speakers still play too**: they're muted while it sends
  its sound, and muted again within a second if something unmutes them. Check
  **Settings → Sound → Mute this PC's speakers while sending** is on there, and its Logs tab
  for "couldn't mute this PC's speakers".
- **The other PC stayed muted**: glide unmutes it when it stops, but if
  glide was killed (Task Manager, a crash) it couldn't. Unmute it from the
  taskbar volume icon.
- The **Logs** tab shows what's happening, and **Copy all** grabs it for a bug report.
