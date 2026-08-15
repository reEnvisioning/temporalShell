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
```

## Config

`${XDG_CONFIG_HOME:-$HOME/.config}/reEnvisioning/temporalShell/config.toml`:

```toml
border_thickness_px = 10
corner_radius_px = 16
```

`border_thickness_px` is 1–256 (default 10); `corner_radius_px` is 0–256 (default 16).
The sharp opaque border has a fixed 3 px inward black shadow (30% → 20% → 10% alpha), which scales with integer buffer scale and is intentionally not configurable.
Requires a Wayland compositor advertising `wlr-layer-shell`; no X11 support.
