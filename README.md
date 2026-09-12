# temporalShell

A passive border and timer line for Wayland compositors with `wlr-layer-shell`.

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
temporalShell timer add 10s
temporalShell timer add --date 2096-02-29T12:34:56Z
temporalShell timer list
```

Both `temporalShell` and `temporalshell` are installed. `available` and every timer command are read-only with respect to config. Run either command with `help` for the full timer command reference.

The shell runs in the foreground without systemd and uses one input-empty, non-exclusive full-output surface and up to two retained ARGB buffers per output. It follows output hotplug and integer buffer-scale changes; a timer range that cannot fit a newly configured output is left transparent there while valid outputs continue. Buffers above 64 MiB are rejected. Drawing requires Linux and a compositor advertising `wlr-layer-shell`; there is no X11 or fractional-scale protocol support.

## Config

The canonical defaults are tracked in [`default.toml`](default.toml) and embedded in the binary. On the first normal shell launch only, an absent `${XDG_CONFIG_HOME:-$HOME/.config}/reEnvisioning/temporalShell/config.toml` is created from those exact bytes. This works on NixOS, Void, Arch, and other Linux distributions; package installation and Home Manager never create it. If the file is absent during a later live reload, compiled defaults are used without recreating it.

The parser rejects unknown or duplicate keys and tables. Root keys must precede `[timer]`. The old root `event_line_thickness_px` key is an alias for `timer.thickness_px`; setting both is an error. Colors must be quoted `"#RRGGBB"`. Border and timer pixels are opaque; the existing inward shadow remains controlled by `shadow_strength_percent` (0–100) and `shadow_color`. Border thickness is 1–256px, corner radius is 0–256px, and timer thickness is 1–256px but must not exceed border thickness.

Timer edges are `top`, `right`, `bottom`, or `left`. Bounds are quoted nonnegative integer `N%` (0–100) or `Npx`; units may be mixed, but the resolved output bounds must remain within the edge and satisfy start < end. Percent bounds use integer floor against the full edge length, so `100%` is its exclusive endpoint. Horizontal coordinates run left to right. Vertical coordinates run from screen bottom to top.

`duration` uses positive descending `d`, `h`, `m`, and `s` components and is capped at `1d`. During that interval only the nearest future timer is shown. It disappears at its deadline, then the next future timer is selected. `expand` grows from the center to both endpoints. `flow-up` and `flow-down` are accepted only on left/right edges and move toward greater/lower local vertical coordinates respectively. `static` shows the full range. `blink` alternates the full range at 2Hz.

With `fade = false`, active timer pixels switch directly between the opaque border and timer colors. With `fade = true`, each pixel starts integer RGB interpolation when its animation front reaches it, over 250ms; all active pixels reverse over the last 250ms before the deadline. Static pixels begin together; blink fades in then out over its two 250ms phases. There is no fade-speed setting.

Config and timer state are checked every 250ms. Invalid startup config is fatal. Reload errors are reported once per distinct error while the last valid config or timer snapshot remains active. Animated transitions use compositor frame callbacks; if both retained buffers are busy, rendering retries after about 16ms instead of freezing. Idle, static, and unchanged blink frames are not redrawn.

## Timer state

`timer add DURATION` accepts values such as `10s`, `30m`, `40h`, or `3d5h3s`. `timer add --date DATE` accepts a strictly future `YYYY-MM-DDTHH:MM:SSZ` UTC timestamp. Add `--id ID` for a case-sensitive ASCII ID matching `[A-Za-z0-9][A-Za-z0-9_-]{0,63}`; otherwise an eight-character lowercase alphanumeric ID is generated. Add and list print `ID<TAB>DATE`, sorted by deadline then ID. `timer remove ID`, `timer remove --date DATE`, and `timer reset` remove state.

State lives under `${XDG_STATE_HOME:-$HOME/.local/state}/reEnvisioning/temporalShell/timers/`. Management commands use private files, atomic publication, and a private lock. The renderer's bounded snapshot is read-only: absent state stays absent and creates no directories or locks. Malformed names, dates, permissions, symlinks, special files, oversized entries, and more than 4096 directory entries are rejected. Remove-by-date and reset can be partially complete after an I/O failure, so retry them.
