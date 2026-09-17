# temporalShell

A passive Wayland border with declarative timer and trigger highlights.

## Install and use

```sh
cargo install --git https://github.com/reEnvisioning/temporalShell.git
nix run github:reEnvisioning/temporalShell -- available
```

```sh
temporalshell
temporalshell available
temporalshell timer add 10m --countdown
temporalshell trigger --edge top --animation static --color '#ffffff'
```

`available`, timer commands, and trigger commands require no Wayland surface. The shell is foreground-only, input-empty, non-exclusive, Linux-only, and needs a compositor advertising `wlr-layer-shell`.

## Config

The embedded [`default.toml`](default.toml) is created only by a first normal shell launch at `${XDG_CONFIG_HOME:-$HOME/.config}/temporalshell/config.toml`. Unknown and duplicate keys are rejected. Highlights contain only geometry, lifetime, animation name, and color:

```toml
[highlight.warning]
edge = "top"
start = "0%"
end = "100%"
duration = "2s"
animation = "blink"
color = "#FF0000"
```

Rounded percentage bounds run from one corner-arc midpoint to the next; signed percentage overflow reaches at most the neighboring edge, while pixel bounds retain their tangent-to-tangent edge-relative offsets.

Every animation is configured under `[animation.NAME]`; names have no built-in behavior. The tracked defaults define `expand`, `flow-up`, `flow-down`, `static`, and `blink`. An animation selects a named `[pixel.NAME]` behavior and may provide keyframes, interpolation, an expression, and a spatial mask:

```toml
[animation.spring]
pixels = "fade"
keyframes = ["0:0", "55:115", "72:94", "86:103", "100:100"]
interpolation = "smooth"
expression = "keyframe + 0.03 * sin(phase * 30)"
mask = "keyframe - position"
```

Keyframes are `time:factor` decimal percentages. Times must increase from 0 through 100; factors may overshoot. Interpolation is `linear`, `smooth`, or `step`. Without keyframes, `keyframe` is 1. With both keyframes and an expression, the expression transforms the interpolated value.

Expressions support finite numbers, parentheses, unary `-`, `+ - * / %`, and `abs`, `min`, `max`, `clamp`, `lerp`, `step`, `smoothstep`, `floor`, `ceil`, `round`, `fract`, `sin`, `cos`, `tan`, `exp`, `log`, `pow`, and `sqrt`. Available values are `phase`, `keyframe`, `from`, `to`, `value`, `elapsed`, `remaining`, `duration`, `position`, `pixel`, `coverage`, `pi`, and `e`. Invalid or non-finite operations become zero rather than entering the raster.

Pixel behavior is independent from movement. The default fade is itself TOML rather than Rust policy:

```toml
[pixel.fade]
expression = "coverage * min(1, min(elapsed, remaining) / 0.25)"
```

Expressions compile when config loads. The renderer performs no parsing while drawing. The 64 KiB complete config bound is the aggregate animation/keyframe bound; there is no separate animation or keyframe-count policy.

The evaluator can carry `from` and `to` separately from the movement factor, but current timers and triggers supply `0` and `1`. A future dynamic source could initialize its first observation with `from == to`, so 40 appears immediately, then evaluate `40 + (45 - 40) * keyframe` for a change to 45. Audio and meter commands are not implemented.

`trigger [--edge EDGE] [--start BOUND] [--end BOUND] [--duration DURATION] [--animation NAME] [--color COLOR]` inherits `[highlight.default]`. The former `fade`, `fade_duration_ms`, `blink_interval_ms`, fixed animation enum, and old trigger record format are intentionally unsupported. Existing config and trigger state are never rewritten or deleted automatically.

State remains private and atomic under `${XDG_STATE_HOME:-$HOME/.local/state}/temporalshell/`; malformed, oversized, unsafe, and old-format records are rejected.
