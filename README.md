# WinForge — Windows tuning in one executable

*Русская версия: [README.ru.md](README.ru.md)*

A desktop app for tuning Windows itself: appearance, privacy, startup
programs, performance. Windows, Tauri 2, and a front end with **no bundler at
all**.

```powershell
cargo build --release --manifest-path winforge/Cargo.toml
```

The result is one `winforge.exe`. `npm` is not needed here at all — the
interface is three plain files in `ui/`, embedded by `tauri-build`. There is
nothing to compile, so there is nothing to break.

## Why no React and no bundler

Because there is nothing to build. The whole interface is lists with
toggles: no shaders, no animation, no charts. React, Tailwind and Vite would
add a second `node_modules`, a second `package.json` and one more way for the
build to fail — all for markup that fits in 600 lines.

## Why `createElement` and not `innerHTML`

This one is not a matter of taste. Half of what the program displays comes
from the registry and the installed-package list: paths, program names,
startup command lines. Glue that into HTML and **a string someone else put in
your startup entry becomes code**, running inside a window that talks to the
registry with administrator rights.

`createElement` with text nodes cannot do that: a string stays a string. The
rule in this codebase is that anything read from the system is inserted as
text, never as markup.

## What it covers

- appearance and shell settings
- privacy: telemetry and data collection switches
- startup programs, with the real command line shown
- performance settings

Written in Rust (Tauri 2). Windows only.
