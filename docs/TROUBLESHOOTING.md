# Troubleshooting

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
- **No sound from another PC**: check **Settings → Sound** on both PCs, and
  that both run glide 0.4.7 or later. The Logs tab on each says whether it's
  sending or playing, or why it can't (no speakers found, say).
- **Sound crackles**: usually Wi-Fi dropouts. glide keeps a little more in
  hand after each one, so it should settle within seconds. A cable helps
  most.
- **The other PC stayed muted**: glide unmutes it when it stops, but if
  glide was killed (Task Manager, a crash) it couldn't. Unmute it from the
  taskbar volume icon.
- The **Logs** tab shows what's happening, and **Copy all** grabs it for a bug report.
