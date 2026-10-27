# Windows

Zornov builds and runs natively on Windows 10 and 11 (x64). Most of it works
as on macOS and Linux; this page covers what is different.

## What you need

- **Rust through [rustup](https://rustup.rs)**, with the MSVC toolchain
  (`x86_64-pc-windows-msvc`, rustup's default on Windows). It needs the
  Visual Studio Build Tools with *Desktop development with C++*; rustup offers
  to install them.
- **The Windows SDK** (the Visual Studio Build Tools install it with *Desktop
  development with C++*). The build copies DXC, the DirectX shader compiler,
  from its `Redist\D3D` folder next to `zornov.exe`; see
  [Graphics](#graphics). To use another copy, set `STUDIO_DXC_DIR` to the
  folder that holds `dxcompiler.dll` and `dxil.dll`.
- **Git.** SQLite, which Windows does not ship, is built from source as part
  of the build.
- **For the installer only:** [WiX 5](https://wixtoolset.org) through the .NET
  SDK: `dotnet tool install --global wix --version 5.0.2`.

## LF line endings

The guides, skills and test fixtures are compiled into the app and read line
by line, and `assets.lock` hashes files as they are committed, so every file
must be checked out with LF endings, as committed. Git for Windows converts
them to CRLF by default (`core.autocrlf=true`); the repository's
`.gitattributes` overrides that, so a plain clone is right:

```cmd
git clone https://github.com/zornov/zornov.git
```

A checkout made before `.gitattributes` existed may still have CRLF files: the
skill parser then panics in the agent tests, and `cargo xtask assets-check`
fails. To fix one, run this once in it. It discards uncommitted changes to
tracked files, so commit or stash them first:

```cmd
git config core.autocrlf false
git rm --cached -r -q .
git reset --hard -q HEAD
```

## Build and run

From Command Prompt:

```cmd
cd /d C:\path\to\zornov
cargo build --release -p studio-app
target\release\zornov.exe
```

The last of the roughly 630 crates is the app itself. It is optimised as a
whole (link-time optimisation), so the build can sit at `628/629` for a long
time on a laptop; that is normal. One build at a time per checkout: a second
`cargo` in the same folder waits with `Blocking waiting for file lock`.

## Two programs: the window and the terminal

On macOS and Linux one `zornov` program does both: started from Finder, the
Dock or a desktop entry it opens the editor, and in a terminal it gives the
prompt. Windows decides when a program starts whether it is a window program
or a console program, so Zornov ships two:

| Program | What it is | Use it for |
|---|---|---|
| `zornov.exe` | Window program | The editor: double click, the Start menu, file associations |
| `zornov-cli.exe` | Console program | The terminal: the prompt, `new`, `export`, `agent` and every other command |

The editor never depends on a terminal. Opened by a double click, the Start
menu or a file, `zornov.exe` starts alone, with no console. Opened with
`zornov-cli app`, it starts as a program of its own, so closing that terminal
or pressing Ctrl+C there leaves the editor open.

For terminal commands, `zornov-cli.exe` starts the `zornov.exe` beside it on
the same console and waits for it, so the two must stay in the same folder. Ctrl+C reaches Zornov
as usual. Every command in [Terminal and headless use](terminal.md) works
with `zornov-cli` in place of `zornov`:

```cmd
zornov-cli new my-video
cd my-video
zornov-cli card colour=red at=0 duration=2
zornov-cli export out.mp4
zornov-cli                    &:: the prompt in this project
zornov-cli app                &:: open the editor window on this project
```

Programs that talk to Zornov over pipes, such as an MCP client running
`zornov mcp`, can start either one. Through `zornov-cli.exe`, the pipes and
files it was given (an MCP client's standard input and output, or
`> out.json`) stay `zornov.exe`'s standard input and output after it
attaches to the console. The Claude
Desktop extension starts `C:\Program Files\Zornov\zornov-cli.exe`; see
[Terminal and headless use](terminal.md#ai-assistants-mcp) for the others.

Neither folder is added to `PATH`. To type `zornov-cli` from anywhere, add
the install folder (or `target\release`) to your user `PATH`.

## Graphics

On Windows, Zornov draws through **DirectX 12, with DXC compiling its
shaders**, whenever `dxcompiler.dll` is next to `zornov.exe`. The build puts it
in `target\release`, and both packages carry it. The other routes fail on
common hardware:

| Route | What happens |
|---|---|
| Vulkan (wgpu's own choice on Windows) | Crashes inside some drivers as the window opens: Intel UHD Graphics with driver 31.0.101.2137 crashes in `igvk64.dll` |
| DirectX 12 with FXC (DirectX's built-in compiler) | FXC cannot compile the `composite` shader, and Zornov stops |
| **DirectX 12 with DXC** | Works |

To choose another route, for example to compare drivers, set `WGPU_BACKEND`
(`vulkan`, `dx12`, `gl`) and `WGPU_DX12_COMPILER` (`dxc`, `fxc`) before
starting Zornov; values set by hand win. If the build printed *DXC was not
found*, that build uses Vulkan.

If the editor never appears, look in `%LOCALAPPDATA%\Zornov\crashes`, and in
Event Viewer under *Windows Logs > Application* (*Application Error*, with
the faulting module).

## Packages

| Command | Makes |
|---|---|
| `cargo xtask package-windows` | `dist\zornov-windows-x64.zip`: unzip anywhere and run, no install |
| `cargo xtask package-msi` | `dist\release\Zornov-<version>-windows-x64.msi`: installs per machine, adds a Start menu entry and upgrades earlier versions |

Each one builds the app first, so `cargo build` beforehand is not needed.
Both carry `zornov.exe`, `zornov-cli.exe`, DXC (`dxcompiler.dll`,
`dxil.dll`, with Microsoft's notice in `DirectXShaderCompiler-NOTICE.md`) and
the licence notices.

- **Signing.** The MSI is signed only when `STUDIO_WIN_SIGN_THUMBPRINT` names
  a code-signing certificate in the Windows certificate store (see
  [Releasing](releasing.md)). An unsigned MSI or zip makes SmartScreen warn
  on first run: choose *More info*, then *Run anyway*.
- **Content library.** Packages do not include it. The app downloads it from
  the Assets and Audio panels.

## Notes for contributors

- A window program that starts a console program gives that program a
  console window of its own. Start console programs from the app with
  `CREATE_NO_WINDOW` (`0x0800_0000`, through
  `std::os::windows::process::CommandExt::creation_flags`), as
  `app/src/agent/procs.rs` does for everything the AI editor starts.
- `zornov.exe` is a window program only in the build itself
  (`cfg_attr(all(windows, not(test)), …)` in `app/src/main.rs`), so
  `cargo test` output still shows in the terminal.
- CI builds, tests and packages Windows on every change (the `windows` job
  in `.github/workflows/build.yml`) but cannot double click anything, so check
  changes to start-up, the console or child programs by hand.
