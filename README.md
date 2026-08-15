# temporalShell

Passive black borders for Wayland compositors that implement `wlr-layer-shell`.

## Install

```sh
nix run github:reEnvisioning/temporalShell
# or: cargo install --path .
```

## Use

```sh
temporalShell             # run in the foreground
temporalShell available   # probe required Wayland capabilities
```

`available` checks for a Wayland display, `wl_compositor`, `wl_shm`, an output,
and `zwlr_layer_shell_v1` without creating surfaces. Errors name the missing
capability. There is no X11 mode.

## Config

Path: `${XDG_CONFIG_HOME:-$HOME/.config}/reEnvisioning/temporalShell/config.toml`

```toml
border_thickness_px = 10
```

The default is 10 logical pixels; accepted values are 1–256. Unknown,
duplicate, malformed, and oversized config is rejected. Restart to reload.

## Compatibility

Works on Niri, Hyprland, and other compositors only when they advertise
`wlr-layer-shell`; Wayland does not guarantee that protocol. Surfaces request
no keyboard input, use empty pointer input regions, and reserve no workspace.
Integer buffer scales are rendered directly; fractional scaling is handled by
the compositor because this client does not implement fractional-scale.

```sh
cargo test
nix flake check
```
