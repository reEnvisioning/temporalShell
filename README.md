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
temporalshell
temporalshell available
```

## Config

`${XDG_CONFIG_HOME:-$HOME/.config}/reEnvisioning/temporalShell/config.toml`:

```toml
border_thickness_px = 10
corner_radius_px = 16
shadow_strength_percent = 100
shadow_color = "#000000"
```

`border_thickness_px` is 1–256 (default 10); `corner_radius_px` is 0–256 (default 16).
`shadow_strength_percent` is 0–100 (default 100) and multiplies the fixed 30% → 20% → 10% three-band inward-shadow ramp; `0` disables the shadow. `shadow_color` is strictly quoted `"#RRGGBB"` ASCII hex (default `"#000000"`); its RGB is premultiplied internally. The border remains sharp, opaque black.
Requires a Wayland compositor advertising `wlr-layer-shell`; no X11 support.
The CLI is distro-neutral across NixOS, Void, and Arch, runs in the foreground, and does not require systemd; start it from a shell, compositor startup, runit, or any supervisor you trust. macOS builds only keep help/parsing/core validation available; drawing borders is Linux Wayland-only.

Run `temporalShell help` (or `temporalshell help`) for the full command reference. `available` probes capabilities without creating surfaces.
