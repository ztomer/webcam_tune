# webcam_tune

Keep a UVC webcam on one set of image settings on macOS, with one profile file per camera.
The first and only profile today is the **Razer Kiyo Pro (v1, USB `1532:0E05`)**.

```bash
curl -fsSL https://raw.githubusercontent.com/ztomer/webcam_tune/main/install.sh | bash
```

Razer Synapse, the only official way to tune the Kiyo Pro, does not run on macOS. The camera
does accept standard UVC commands, but it forgets what it was told when it is unplugged,
power-cycled or the Mac sleeps, and every app that opens it may leave it changed. `webcam-tune`
writes your settings to the camera over USB, reads each one back to prove it took, and
re-applies them every time the camera is plugged in and every time the Mac wakes.

- **One small binary.** Rust, talking UVC directly over USB: no `uvc-util`, no Python, no
  vendor software.
- **Event-driven.** The agent sleeps on kernel notifications (each profile's camera appearing
  on USB, and the Mac waking) and uses no CPU between them. Measured on macOS 27.2: 2.4 MB
  physical footprint (Apple's own `caffeinate` uses 1.8 MB; 0.3 MB of the difference is the
  time-zone database behind local-time log lines), 0.02 s of CPU in its first minutes.
- **One file per camera, nothing hidden in the binary.** Every setting the program uses lives
  in the profile; a missing key is an error that names it.

Apple silicon Macs only. The installer refuses anything else, because nothing here has ever run
anywhere else.

## Install

The one-liner above needs `git` (Xcode command line tools) and Rust
([rustup.rs](https://rustup.rs)). It clones this repo into `~/.local/share/webcam_tune/src` at
its newest release tag, then:

- builds and installs `~/.local/bin/webcam-tune`,
- copies each profile in `profiles/` to `~/.config/webcam_tune/profiles/` **unless you already
  have one by that name** (your edits are never overwritten),
- loads a LaunchAgent that runs `webcam-tune watch` at login, logging to
  `~/Library/Logs/webcam-tune.log`.

Run the same line again to update. From a clone, `./install.sh` does the same with that checkout.

**Already tuned the camera and want to keep it?** Run `webcam-tune capture` after installing. It
rewrites the profile's `[controls]` with what the camera holds now.

Uninstall (your profiles stay): `~/.local/share/webcam_tune/src/install.sh --uninstall`.

## Use

```text
webcam-tune apply        write the profile's [controls] to its camera and read every value back
webcam-tune capture      rewrite [controls] with what the camera holds now
webcam-tune lock focus   freeze the autofocus position as manual focus, then capture
webcam-tune lock wb      freeze the auto white balance as a manual value, then capture
webcam-tune show         the camera's value and the profile's, control by control
webcam-tune watch        apply each profile when its camera is plugged in, all of them on wake
webcam-tune --help | --version
```

`--profile <name|path>` picks a profile (`kiyo-pro`, or a `.toml` path); with one profile
installed it can be left out. `--profiles-dir <dir>` points at another directory. Exit status: 0
done, 1 the camera did not take the profile (each control is named), 2 bad usage.

To tune: adjust the picture in any UVC app, or edit `[controls]` and run `webcam-tune apply`.
Run `capture` once you like it. The agent re-reads the profiles on every event, so edits take
effect at the next plug-in or wake. A new profile file is picked up when the agent restarts
(log out and in, or re-run the installer).

## Profiles

One TOML file per camera in `~/.config/webcam_tune/profiles/`, named by its file stem. Every key
is required:

```toml
name = "Razer Kiyo Pro (v1)"            # shown in every log line about this camera
description = "Factory picture with the community's fixes"
version = "1.0.0"                       # MAJOR.MINOR.PATCH; capture and lock bump the patch
updated = 2026-11-01                    # a TOML date; capture and lock set it to today

[device]
vendor-id = 0x1532                      # which camera this profile is for
product-id = 0x0E05

[agent]
settle-ms = 1500          # gather a burst of plug-in and wake events this long, then apply once
ready-timeout-ms = 10000  # how long a camera that has just appeared gets to start answering
retry-ms = 250            # how often to retry within that window
usb-timeout-ms = 500      # the deadline for one USB control request

[controls]
power-line-frequency = 2  # anti-flicker: 1 = 50 Hz mains, 2 = 60 Hz (North America)
backlight-compensation = 0
auto-exposure-mode = 8    # 8 = automatic, 1 = manual (then exposure-time-abs applies)
auto-exposure-priority = 0
auto-white-balance-temp = true
auto-focus = true
brightness = 128
# ... see profiles/kiyo-pro.toml for every control
```

Controls you leave out are left alone. The program sets the order itself: an auto mode is
always written before the manual value it governs, because the camera refuses a manual
exposure, white balance or focus while its auto mode is on. A manual value whose auto mode the
profile leaves on is skipped with a warning rather than sent to be refused. Two profiles for the
same camera are refused.

To add a camera, copy a profile, set its `[device]` ids (`ioreg -p IOUSB -l | grep -E
'"USB Product Name"|idVendor|idProduct'` lists them), and send a pull request.

## What the community recommends for the Kiyo Pro

| Recommendation | Why | In the shipped profile |
|---|---|---|
| Light your face from the front | The biggest single win; the STARVIS sensor is good, auto-exposure in a dim room is not | (lighting is on you) |
| Turn "HDR" off unless you are backlit | It caps the camera at 30 fps and flattens the image | `backlight-compensation = 0` |
| Hold the frame rate | In dim light auto-exposure otherwise slows the camera down to expose | `auto-exposure-priority = 0` |
| Match anti-flicker to your mains | Wrong value = banding under LED or fluorescent light | `power-line-frequency = 2` (60 Hz); set `1` for 50 Hz |
| Lock focus | Autofocus hunts when you move | `webcam-tune lock focus` while sitting normally |
| Lock white balance under fixed lighting | Auto drifts with what is on your screen | `webcam-tune lock wb` |
| Lower sharpness if edges halo | The factory value over-sharpens | `sharpness` (factory 128) |
| Plug into a USB 3 port directly | 1080p60 needs it; hubs cause lag | (cabling is on you) |
| Update the firmware once | Razer's updater is Windows-only | (one-time, on a PC) |

## Limits, stated plainly

- **Kiyo Pro field of view (wide/medium/narrow) and Razer's own HDR mode are not exposed.** The
  camera implements them in two Razer extension units whose protocol is undocumented; nothing is
  sent to them. `zoom-abs` crops digitally instead.
- **Firmware updates are out of scope**, and always will be: a wrong command can brick a camera.
- Tested on one camera (Kiyo Pro), macOS 27.2, Apple silicon. For the Kiyo Pro Ultra (`0E08`)
  and Kiyo V2 Pro (`0E0A`) with their Razer-specific controls, see
  [Kiyo Control](https://github.com/f0ns/kiyo-control).

## Development

```bash
cargo test               # unit tests (a fake camera that behaves like the measured one),
                         # the binary as a user runs it, and the installer end to end
./tools/gate.sh --full   # the full gate: fmt, pedantic clippy, audit, coverage floor
```

`unsafe` lives in `src/notify.rs` only (the IOKit plug-in and wake notifications), held there by
`tests/unsafe_scope.rs`.

## License

MIT, see [LICENSE](LICENSE).
