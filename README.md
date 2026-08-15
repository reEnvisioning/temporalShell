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
```

Requires a Wayland compositor advertising `wlr-layer-shell`; no X11 support.
