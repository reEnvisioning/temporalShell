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
temporalshell
temporalshell timer add --date 2096-02-29T12:34:56Z --id meeting
temporalshell timer add 10m --countdown
temporalshell trigger warning
temporalshell trigger --edge top --start 0% --end 100% --thickness-px 1 --duration 10s --animation static --fade false --color '#ffffff'
temporalshell timer list
```

Only the lowercase `temporalshell` command is installed. `available`, timer commands, and trigger commands never create config or require Wayland. Run `temporalshell --help` for the exact command reference.

The shell runs in the foreground without systemd and uses one input-empty, non-exclusive full-output surface and up to two retained ARGB buffers per output. It follows output hotplug and integer buffer-scale changes; a style that cannot fit one output is skipped there without affecting other styles or outputs. Buffers above 64 MiB are rejected. Drawing requires Linux and a compositor advertising `wlr-layer-shell`; there is no X11 or fractional-scale protocol support.

## Config

The canonical defaults are tracked in [`default.toml`](default.toml) and embedded in the binary. On the first normal shell launch only, an absent `${XDG_CONFIG_HOME:-$HOME/.config}/reEnvisioning/temporalShell/config.toml` is created from those exact bytes. Package installation, management commands, and reloads never create it.

The parser rejects unknown or duplicate keys and tables. Root keys must precede `[timer]`. The old root `event_line_thickness_px` key is an alias for `timer.thickness_px`; setting both is an error. Colors must be quoted `"#RRGGBB"`. Border and timer pixels are opaque; the existing inward shadow remains controlled by `shadow_strength_percent` (0–100) and `shadow_color`. Border thickness is 1–256px, corner radius is 0–256px, and timer thickness is 1–256px but must not exceed border thickness.

Timer edges are `top`, `right`, `bottom`, or `left`. Bounds are quoted nonnegative integer `N%` (0–100) or `Npx`; units may be mixed, but resolved bounds must remain within the edge and satisfy start < end. Percent bounds use integer floor against the full edge length. Horizontal coordinates run left to right. Vertical coordinates run from screen bottom to top.

`duration` uses positive descending `d`, `h`, `m`, and `s` components and is capped at `1d`. `expand` grows from the center to both endpoints. `flow-up` and `flow-down` require left/right edges. `static` shows the full range. `blink` alternates the full range at 2Hz.

`[timer]` is the timer style. Named `[highlight.NAME]` tables must contain all eight timer keys (`edge`, `start`, `end`, `thickness_px`, `duration`, `animation`, `fade`, and `color`); `[highlight.default]` is invalid. `trigger NAME` copies the named style into private trigger state, so later config changes do not alter that trigger. The direct trigger flags copy the same complete style. Active timers and triggers are sorted by start then ID and painted old to new; newer pixels overlay older ones.

## State

`timer add DURATION [--id ID]` and `timer add --date DATE [--id ID]` hide until their deadline, then use `[timer]` for its configured duration. A normal timer whose deadline predates a shell start never appears in that shell; deadlines after startup can use the full duration. Add `--countdown` immediately after the duration or date to animate from creation through the deadline instead. Both expire; no timer highlight flag exists. `timer prune` silently removes expired normal and countdown records. Add and list print `ID<TAB>DATE`; remove and reset remain silent. Legacy one-line `DATE\n` records remain normal timers.

`trigger NAME` uses a named style. The only direct form is `trigger --edge EDGE --start BOUND --end BOUND --thickness-px N --duration DURATION --animation ANIMATION --fade BOOL --color COLOR`; all flags are required in that order. Names are case-sensitive ASCII `[A-Za-z0-9][A-Za-z0-9_-]{0,63}`; automatic IDs are eight lowercase alphanumeric characters. Trigger state is independent under `${XDG_STATE_HOME:-$HOME/.local/state}/reEnvisioning/temporalShell/triggers/`; timer state remains under `timers/`. Management uses private files, atomic publication, and separate private locks. Renderer snapshots are read-only and bounded; absent state creates no directories or locks. Malformed names, state, permissions, symlinks, special files, oversized entries, and more than 4096 directory entries are rejected.
