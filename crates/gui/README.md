# hatchzip-gui

The HatchZip desktop app, built with [Slint](https://slint.dev) under its Royalty-free 2.0 license. It drives the same engine as the `hatchzip` command, through `hatchzip-core`'s `session` and `engine` modules.

```sh
cargo run -p hatchzip-gui                         # main window
cargo run -p hatchzip-gui -- game.zip             # main window with an archive open
cargo run -p hatchzip-gui -- --extract-here game.zip
cargo run -p hatchzip-gui -- --extract-to game.zip
cargo run -p hatchzip-gui -- --test game.zip
```

The three task forms are what file manager menus call, so they don't change.

## Drawing

| | |
|---|---|
| Style | Picked at build time in `build.rs`: Fluent on Windows and Linux, Cupertino on macOS. `SLINT_STYLE` overrides it. |
| Renderer | FemtoVG (OpenGL) first. Without a working OpenGL driver the app restarts itself with Slint's software renderer, see `src/backend.rs`. `SLINT_BACKEND=winit-software` forces the software renderer. |
| Theme | Light or dark, following the system. |

Files dropped on the window open on Windows, macOS and X11. winit 0.30 doesn't report drops on Wayland.

## Tests

`cargo test -p hatchzip-gui` runs the unit tests and `tests/ui.rs`, which drives the real windows headlessly through Slint's testing backend. Set `HATCHZIP_GUI_SHOTS=<dir>` to also save every state it passes through as a PNG, in light and dark.
