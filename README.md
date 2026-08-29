# temporalShell

Passive black borders for compatible Wayland compositors.

## Install

```sh
cargo install --git https://github.com/reEnvisioning/temporalShell.git
nix run github:reEnvisioning/temporalShell -- available
```

```nix
inputs.temporalshell.url = "github:reEnvisioning/temporalShell";
packages.${pkgs.system}.default = inputs.temporalshell.packages.${pkgs.system}.default;
```

## Use

```sh
temporalShell
temporalShell available
temporalShell timer add 3d5h3s
temporalShell timer add --date 2096-02-29T12:34:56Z
temporalShell timer list
temporalshell
temporalshell available
temporalshell timer add 10s
```

## Config

`${XDG_CONFIG_HOME:-$HOME/.config}/reEnvisioning/temporalShell/config.toml`:

```toml
border_thickness_px = 10
event_line_thickness_px = 3
corner_radius_px = 0
shadow_strength_percent = 35
shadow_color = "#B8A890"
```

`border_thickness_px` is 1–256 (default 10). `event_line_thickness_px` is 1 through the border thickness and defaults to 3, or to the border thickness when that is smaller; it is stored for a future event line and does not change the current black border. `corner_radius_px` is 0–256 (default 0).
`shadow_strength_percent` is 0–100 (default 35) and multiplies the fixed 30% → 20% → 10% three-band inward-shadow ramp; `0` disables the shadow. `shadow_color` is strictly quoted `"#RRGGBB"` ASCII hex (default `"#B8A890"`); its RGB is premultiplied internally. The default is a 10px opaque black square frame with this inward shadow. One full-output surface and one retained ARGB buffer are used per output; buffers larger than 64 MiB are rejected. Event-line and timer rendering remain absent.
Requires a Wayland compositor advertising `wlr-layer-shell`; no X11 support.
The CLI is distro-neutral across NixOS, Void, and Arch, runs in the foreground, and does not require systemd; start it from a shell, compositor startup, runit, or any supervisor you trust. macOS builds only keep help/parsing/core validation available; drawing borders is Linux Wayland-only.

Run `temporalShell help` (or `temporalshell help`) for the full command reference. `available` probes capabilities without creating surfaces.

`timer add DURATION` accepts positive descending `d`, `h`, `m`, and `s` components such as `10s`, `30m`, `40h`, or `3d5h3s`. `timer add --date DATE` accepts only strictly future UTC timestamps in `YYYY-MM-DDTHH:MM:SSZ` form; `timer remove --date DATE` accepts canonical UTC dates including past dates. Add `--id ID` after the duration or date for a case-sensitive ASCII ID matching `[A-Za-z0-9][A-Za-z0-9_-]{0,63}`; otherwise it generates a random eight-character lowercase ASCII alphanumeric ID such as `hsr2fx8w`. Custom numeric IDs do not influence generated IDs. Add and list print `ID<TAB>DATE`; list sorts by deadline then ID. `timer remove ID` or `timer remove --date DATE` removes timers, and `timer reset` clears timers plus stale temporary files. State is private under `${XDG_STATE_HOME:-$HOME/.local/state}/reEnvisioning/temporalShell/timers/`, has no visual effect, and needs neither Wayland nor a running shell. Add/list/remove/reset hold a private `.timers.lock`; remove-by-date and reset can be partially complete after an I/O failure, so retry them.
