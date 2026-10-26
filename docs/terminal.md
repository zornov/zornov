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
Zornov. Add it to the assistant's MCP settings:

```json
{
  "mcpServers": {
    "zornov": { "command": "zornov", "args": ["mcp", "--project", "/path/to/my-video"] }
  }
}
```

If that project is open in a window, the assistant works in the window.
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
