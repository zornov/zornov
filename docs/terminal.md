# Terminal and headless use

Everything the editor window can do, Zornov can also do from a terminal or
with no window at all: make a project, edit it, look at it, check it and
export it. Scripts, tests, render machines and AI assistants all use this.
The window and the terminal share the same projects and the same tools, so
you can start in one and finish in the other.

## The prompt

Run `zornov` in a project folder for a prompt. A line that starts with a
command runs it; anything else is said to the AI editor, which works in the
same project with the same tools.

```bash
zornov new my-video          # a new project in ./my-video
cd my-video
zornov                       # the prompt in this project
```

On Windows, `zornov.exe` is the window program (a double click or the Start
menu opens the editor), so use `zornov-cli` beside it in a terminal:
`zornov-cli new my-video`, `zornov-cli` for the prompt, and the same for every
command below. Programs that talk to Zornov over pipes, such as an MCP
client running `zornov mcp`, can use either.

`zornov new` takes `--vertical`, `--square`, `--size 1080x1920` and
`--fps 60`. The project is the folder you are in, or the nearest one above it
that holds a project.

## One command at a time

Any prompt command also runs on its own and exits, which is what scripts
want:

```bash
zornov card colour=red at=0 duration=2
zornov text "Hello" at=0.5
zornov import ~/Movies/beach.mov
zornov place beach.mov at=2
zornov animate clip=9 property=x keys=[[0,0],[1,500]]
zornov summary               # the timeline as JSON
zornov check                 # what is wrong with the edit, and how to fix it
zornov export out.mp4        # renders, then runs a quality check on the file
zornov make "a 10 second intro about the ocean"
zornov app                   # open the editor window on this project
```

Fields are `key=value`. Values that read as JSON (numbers, `true`,
`[1, 2]`) are taken as JSON, and the first field can go without its key.
Every edit returns what changed as JSON, read back from the timeline.

| Group | Commands |
|---|---|
| Look | `summary`, `script`, `see`, `listen`, `inspect CLIP`, `pick X Y`, `frame T`, `check`, `media`, `catalogue PANEL [QUERY]` |
| Change | `import`, `text`, `card`, `place`, `set`, `animate`, `trim`, `split`, `delete`, `move`, `transition`, `undo`, `redo` and every other edit |
| Deliver | `export [PATH]` |
| Learn | `guide [TOPIC]`, `help [COMMAND]` |

`zornov help` lists them all and `zornov help COMMAND` shows one command's
fields.

## Headless: JSON in, JSON out

`zornov agent` calls one tool, or reads one call per line from standard
input with `-`. Each answer is one line of JSON.

```bash
zornov agent --headless --project my-video summary
zornov agent --headless --project my-video edit '{"op": "text", "text": "Hi", "at": 1}'
echo '{"tool": "frame", "args": {"t": 2.0}}' | zornov agent --headless --project my-video -
```

`--out DIR` sets where pictures and renders go. Without `--headless`, if the
project is already open in a Zornov window, the call runs there and you see
it happen.

## AI assistants (MCP)

`zornov mcp` serves the same tools over the Model Context Protocol on
standard input and output, so an AI assistant that speaks MCP can edit with
Zornov. With `--project DIR` the assistant works in that project (an empty
or new folder becomes a new project); without it, the assistant opens or
starts one itself. If the project is open in a Zornov window, the assistant
works in the window and you see each edit happen.

Every setup below starts the Zornov you installed, so install Zornov first.
Assistants that run `zornov` by name need it on their `PATH`:

| System | Where Zornov is | To run it as `zornov` |
|---|---|---|
| macOS | `/Applications/Zornov.app/Contents/MacOS/zornov` | `sudo ln -sf /Applications/Zornov.app/Contents/MacOS/zornov /usr/local/bin/zornov` |
| Linux | `/usr/bin/zornov` from the `.deb`; anywhere for the AppImage or the archive | Nothing to do for the `.deb`; otherwise give the full path |
| Windows | `zornov-cli.exe` in `C:\Program Files\Zornov` (the MSI) or the unzipped folder | Add that folder to your user `PATH`, or give the full path to `zornov-cli.exe` |

On Windows, point assistants at `zornov-cli.exe`, the console program: it
starts `zornov.exe` with the assistant's pipes (see [Windows](windows.md)).
`zornov.exe` itself also works over pipes, so a setup that runs `zornov` by
name from `PATH` works too.

### Claude Code

Zornov's repository is a Claude Code plugin marketplace. In Claude Code:

```text
/plugin marketplace add zornov/zornov
/plugin install zornov@zornov
```

Claude Code 2.1.275 and later also take one step,
`/plugin install zornov --marketplace zornov/zornov`, and the same commands
run in a shell as `claude plugin …`. The plugin runs `zornov mcp` with no
project: ask Claude to open or start one, or open it in the Zornov window
first. To give Claude one project instead, skip the plugin and add the server
yourself:

```bash
claude mcp add zornov -- zornov mcp --project /path/to/my-video
```

### Claude Desktop

Download `Zornov.mcpb`, the Claude Desktop extension, from the
[latest release](https://github.com/zornov/zornov/releases/latest/download/Zornov.mcpb)
and open it: Claude Desktop installs it and asks for the project folder. The
extension holds no copy of Zornov; it starts the one installed in the usual
place, `/Applications/Zornov.app` on macOS or `C:\Program Files\Zornov` (the
MSI) on Windows. For Zornov anywhere else, add it to Claude Desktop's
`claude_desktop_config.json` (*Settings > Developer > Edit Config*) as for
[other assistants](#other-mcp-clients), with the full path.

`cargo xtask package-mcpb` makes the extension, as
`dist/release/Zornov-<version>.mcpb`, from `packaging/mcpb/manifest.json`.

### Codex

Codex keeps MCP servers in `~/.codex/config.toml`, shared by the Codex CLI,
the IDE extension and the ChatGPT desktop app. Add Zornov from a terminal:

```bash
codex mcp add zornov -- zornov mcp --project /path/to/my-video
```

or write the entry yourself:

```toml
[mcp_servers.zornov]
command = "zornov"
args = ["mcp", "--project", "/path/to/my-video"]
```

On Windows, `command = 'C:\Program Files\Zornov\zornov-cli.exe'` (single
quotes keep the backslashes). Leave `--project` out of `args` to let Codex
open or start the project itself.

### Other MCP clients

Any client that starts a local (stdio) server takes the same command. Most
read a JSON entry like this one:

```json
{
  "mcpServers": {
    "zornov": { "command": "zornov", "args": ["mcp", "--project", "/path/to/my-video"] }
  }
}
```

On Windows, `"command": "C:\\Program Files\\Zornov\\zornov-cli.exe"`.

Which AI provider you use, and what it is sent, is up to you and that
provider; see [AI crew](ai-crew.md).

## Renders on other machines

```bash
zornov queue list            # the render queue
zornov queue run             # render everything waiting
zornov node                  # let other Zornov apps send this machine parts of a render
```

`zornov node` takes `--listen ADDR:PORT`, `--token SECRET`, `--name NAME` and
`--work DIR`. It prints a pairing token on start. The Zornov that sends work
adds the node with that token.

## Scored runs

`zornov eval --out DIR` runs the AI editor on a set of scored jobs with no
window, for comparing models and catching regressions.
