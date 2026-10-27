# Seeing what agents go through

Agents drive Zornov through its tools (MCP), and until now the people
working on Zornov only found out what went wrong by reading transcripts
afterwards. These tools close that loop: every call is recorded, the
friction in the records is found and ranked for you, any recorded call can
be run again on the current build, and a coding agent in this repository can
watch an agent inside the app as it works.

All of it is local. Nothing is sent anywhere, and nothing listens on the
network.

| | release build | dev build (`cargo run`) | dev build with `--features agent-dev` |
|---|---|---|---|
| flight recorder | off, `zornov trace on` to opt in | on | on |
| `zornov trace report`, `zornov replay` (on a copy) | yes | yes | yes |
| `feedback` tool | yes | yes | yes |
| live tap `zornov mcp --observe` | not built | not built | yes |
| two-way `zornov dev ask / retry` | not built | not built | yes |
| hot-swapped tools `zornov dev swap` | not built | not built | yes |

`agent-dev` is a cargo feature that release builds never turn on, and a
build with it in release mode does not compile (`compile_error!` in
`app/src/agent/dev.rs`). In a build without it, the tap, the two-way channel
and the hot-swap loader do not exist, so no setting can turn them on.

```bash
cargo run -p studio-app --features agent-dev
```

## The flight recorder

Every tool call, from every source, is written down as one line of JSON:

| field | what it is |
|---|---|
| `source` | `stdio` (`zornov mcp`), `attach` (a client in the window through its socket), `panel` (the AI panel), `eval`, `agent` (`zornov agent`) |
| `tool`, `ops`, `args` | the call; `args` are redacted (below) |
| `ok`, `error` | whether it worked, and the error text |
| `ms` | how long it took |
| `reply_bytes`, `reply_tokens`, `images` | how big the answer was; tokens are estimated (text at about 4 bytes a token, pictures at width × height / 750) |
| `rev_before`, `rev_after` | the project's revision before and after |
| `clips` | the clip ids it `added`, `removed` and `changed` |
| `reply` | the parts of the answer the report and replay compare: notes, the read-back, `check`'s problems |
| `module` | the Rust module that answered (`agent::quality`, `agent::mod (edit: set, animate)`, `zornov_agent_hot (hot-swapped)`) |
| `seq` | the call's number in this process (`zornov dev retry` takes it) |
| `record_us`, `queue_us` | what recording cost the call, and how long the line waited for the writer (and so the live tap) |

Lines go to `.zornov/agent-trace/<session>.jsonl` in the project folder, or
`agent-trace/` in Zornov's cache when there is no project. A session is one
process (`<start time>-<pid>`); `zornov eval` starts one for each task.

**Redaction.** These are replaced by `[redacted N bytes]`:
- media and file contents: keys such as `data`, `bytes`, `content`, `source`,
  `pixel` and `wgsl`, `data:` URLs, long base64-looking strings, and any
  string over 4 000 bytes;
- secrets: any key containing `secret`, `token`, `password`, `api_key`,
  `authorization`, `cookie`, `credential` or `private_key`;
- anything that looks like a key: `sk-…`, `ghp_…`, `xoxb-…`, `AKIA…`, JWTs
  (`eyJ…`), `Bearer …`, `-----BEGIN …`, and long single words of letters
  and digits.

Errors and notes have the same words taken out, for example
`Authorization: Bearer …` and `token=…`. Paths, ids, times and the words of
titles stay, so a call can be replayed. Pictures in replies are counted,
never kept. Arguments still over 64 KB after redaction are kept as their
size.

**Files.** Traces are `0600` in a `0700` folder on Unix, and owner-only on
Windows. A file is rotated at 16 MB, with at most 8 kept per session and
256 MB per folder, oldest first.

**Cost.** The call hands its sizes, its arguments, the first text of the
reply and a shared pointer to the view before and after to one writer
thread over a channel. Building the line, redacting, comparing clips,
writing and telling the taps all happen there, and writes are buffered and
flushed when the writer is idle. With the recorder off and no tap, a call
pays two atomic loads.

**On or off.** On by default in dev builds (`cargo run`), off in release
builds unless switched on:

```bash
zornov trace on
```

`zornov trace off` switches it off and `zornov trace status` says which.
`ZORNOV_TRACE=1` or `ZORNOV_TRACE=0` wins over both for one run. Unit tests
never record.

The recorder lives in `app/src/agent/trace.rs` and is called once, from
`Session::run`. Undo and redo in the window, which the window answers
itself, are recorded from `live.rs`.

### Agent feedback

Agents also get a `feedback` tool: `expected`, `got`, and optionally `tool`,
`kind` (`surprise`, `ignored_field`, `unclear_error`, `missing`, `docs`,
`slow`, `intended`) and `finding`. It changes nothing and costs one short
reply. Its text is capped at 2 000 bytes a field, and appending it to the
trace is all it does. It goes into the trace like any call, and the report lists it. An
agent that thinks a `check` finding is intended says so with
`kind: "intended"`, and the report counts that finding as noise.

## The report

```bash
zornov trace report my-project/.zornov/agent-trace
zornov trace report session.jsonl --out friction.md
zornov trace report traces/ --json
```

It takes one trace or a folder, searched to any depth, and finds:

- **an error, then a retry** of the same tool within three calls with
  different fields: usually a gap in the vocabulary or the docs. The example
  shows the error and what the agent changed (`-ops[0].txt`,
  `+ops[0].text="Hi"`);
- **silent drops**: an op the read-back says left its clip exactly as it was,
  a field sent in an edit that the read-back of its clip never mentions, or a
  note saying a field was ignored;
- **an edit, then `undo`** within three calls: a bad result;
- **the same read again** (summary, inspect, frame, and so on) with the same
  arguments and nothing changed in between;
- **oversized replies** (over 4 000 tokens) and **slow calls** (over 5 s);
- **`check` noise**: findings still there in the session's last `check`
  after the agent saw them in its first one, or that the agent called intended;
- **agent feedback**.

The report is markdown. A table ranks the tools by a weighted score
(retries and silent drops count most), then each tool has a section with
each kind of friction, how many times it happened, what it usually means
and one example call (`session`, call number). The `--json` output has the
same findings and the per-session metrics that `zornov eval` uses.

The detector is in `app/src/agent/trace_report.rs`. The thresholds are
constants at the top of the file.

## Replay

```bash
zornov replay session.jsonl --project my-project
zornov replay my-project/.zornov/agent-trace --session 1761550000-4242 --call 37
zornov replay session.jsonl --upto 20 --emit-test
```

This runs a recorded session (or `--call N`, or the first `--upto N` calls)
again on this build and compares each outcome with the recorded one:
whether it worked, the error, whether it changed the project, the read-back
of its clips (compared without clip ids, since a copy numbers its clips
afresh) and the ops the read-back said did nothing. It prints `same`,
`DIFFERS` with the differences, or `skipped` with why, and exits 1 when any
call differs, so it can gate a change.

The calls run on a copy of `--project` in the temp folder, or on a blank
project when no project is given, with the recorder off. Your project is
never touched. For a faithful replay, give a copy of the project as it was
when the session began (`versions` keeps snapshots).

A trace is untrusted input: anyone could hand you one. Replay checks every
call before it runs it:
- only tools that stay inside the project run: edit, undo and redo, and the
  reads and looks;
- no op that reaches outside, such as voices, generation, scripts, plugins
  or `raw`;
- the arguments must be an object under 256 KB, and not redacted;
- every path must be inside the copy. A path into the original project is
  moved into the copy. A call with any other absolute path, `~`, `..` or a
  URL is skipped and says why.

Replay never runs a shell or an external program, and never fetches
anything. The project copy does not follow symbolic links. The report and
replay read at most 256 MB per file and 1 MB per line, and do not follow
links when searching a folder. Fixtures are checked the same way.

**`--emit-test`** writes every call that failed in the recording as a JSON
fixture in `app/tests/agent_replays/`: the earlier calls that changed the
project (the setup), the call, and what it does on this build from a blank
project (`expect`). The test `recorded_agent_calls_still_behave` in
`app/src/agent/replay.rs` runs every fixture with `cargo test`. The fixture
records current behaviour. When you fix the tool, change `expect` to what it
should now do, and the test keeps it that way.

## The live tap (agent-dev)

```bash
zornov mcp --observe --text          # the window opened last
zornov mcp --observe --project my-project
zornov mcp --observe "$TMPDIR/studio-agent-123/agent.sock"
```

This follows an open window's calls as they happen, whatever makes them: the
AI panel, Claude Code attached to the window, or `zornov agent`. Each call is
one trace line on stdout, with `module` naming the Rust module that answered
it. `--text` prints one readable line per call instead, plus the error, or
the expected and got of a `feedback` call. It works whether or not the
recorder is writing to disk.

A headless `zornov mcp --project DIR` on stdio has no window, so in
agent-dev builds it opens a listen-only socket of its own,
`$TMPDIR/studio-tap-PID/agent.sock`, and names it on stderr (`live tap
at …`). That socket answers nothing but `{"observe": true}`: any other
first line closes the connection.

Lines are pushed the moment the writer builds them, with no polling. A
call is on the observer's screen well under 100 ms after it ends, usually
under 1 ms. `--text` shows that delay on each line (`+N ms`).

It goes through the window's agent socket:
- **Unix:** `$TMPDIR/studio-agent-PID/agent.sock`, in a `0700` folder, the
  socket `0600`, and every connection is dropped unless the peer process is
  your user (`SO_PEERCRED` on Linux, `getpeereid` on macOS).
- **Windows:** 127.0.0.1 only, on a random port, with a 256-bit token from
  the OS random generator, compared in constant time. The token file is
  created with an owner-only ACL. After the first line,
`{"observe": true}`, the window only writes to that connection and never
reads it, so an observer cannot run a tool or change the project. A coding
agent in this repository can run the command in the background (Claude
Code's Monitor tool, or `| jq` in a terminal) and see what an agent inside
the app is doing, then fix the module the trouble comes from.

## Two-way contact and hot-swapped tools (agent-dev)

A coding agent in this repository can do more than watch:

```bash
zornov dev ask "Use the colour op instead of grade; grade was renamed."
zornov dev retry 41          # call 41 again, through the gate, and what differs
zornov dev swap --retry 41   # rebuild crates/dev/agent-hot, load it, retry 41
zornov dev status
```

- **`ask`** reaches the agent inside as a correction, the same path as a
  person typing one in the panel, before its next call. It needs a panel
  request to be running, and its text is capped at 2 000 characters.
- **`retry N`** runs recent call N again (`seq` in the tap) as an ordinary
  tool call through the window's gate. Pause, Stop, the step limit and
  approvals all apply, and it is recorded with source `dev`. It then says
  what differs from the first time. Only the window's last 256 calls can be
  retried, from memory, never from a file. The agent inside does not wait:
  the retry is one more call in the queue.
- **`swap`** runs `cargo build -p zornov-agent-hot` in this checkout, which
  takes seconds, because the crate is small and has no dependency on the
  169k-line `app` crate. It sends the library's SHA-256 to the window, and
  the window loads it and retries the call.

All requests are rate-limited to five at once, then one a second.

**What the window will load.** Only `target/debug/` of the checkout the
running binary was built from. The path is fixed at build time and never
taken from a request or a trace. The file must be yours and writable by you
alone, inside the checkout with no link out of it, and match the SHA-256
just built. The hashed bytes are written to a private copy, and that copy
is what is loaded, so the file cannot change between check and load.

A library speaks a small C ABI (version 1) and may only take over tools the
app already lists, or reword the arguments before the app's own tool
checks them. It never gets the project, so every change still goes through
`edit` and its checks and undo. Old libraries stay loaded, so a call in
flight finishes, and the newest answers.

So far, `crates/dev/agent-hot` holds `feedback` and the argument aliases
for `edit` ops (`txt` → `text`, `color` → `colour`). Those aliases fix the
vocabulary gaps that `zornov trace report` finds in seconds. Each alias
moves into `app/src/agent/args.rs` once it has proved itself. To move next:
`edit_schema`'s op checks, `capabilities`, `howto`, and the op field tables
in `args.rs`, which are pure data and functions. Tools that need the
project (edit ops, `check`, `frame`) stay in `app`. Moving them needs a
read-only view of the project in the ABI.

## Agents inside the app

`app/src/agent/inproc.rs` gives an agent running in the app the engine
without the wire:
- `Inside::call` goes through the same gate as every call, with no other way
  in. Its `Reply` gives the JSON parsed once and the pictures as JPEG bytes
  in memory, never base64.
- `Inside::frame(t)` returns the picture at `t` seconds as bytes.
- `Inside::project()` returns the project model itself, read-only (an `Arc`
  the window shares when its revision moves, and only while an inside agent
  reads it). It is refused after Stop. Changes still go through `edit`.

The API brain and the crew still take JSON replies. Moving them onto
`Inside` comes next.

## Connection speed

`zornov mcp` (headless, over stdin and stdout) adds three things to plain
MCP. They are in `app/src/agent/mcp_extras.rs`:

- **Export progress.** An `export` call with `_meta.progressToken` renders in
  the background and sends `notifications/progress` (0 to 100, with the time
  left) until it finishes, then answers with the file. Without a token,
  nothing changes. The answer leaves out the automatic QC and loudness of a
  blocking export and points to `qc` instead.
- **Project state as resources.** `studio://project/summary` and
  `studio://project/media` can be read and subscribed to (`resources/subscribe`).
  After any call that moves the revision, subscribers get
  `notifications/resources/updated`, so an agent re-reads only when the
  project changed instead of calling `summary` to find out.
- **Pictures as files.** A local client that asks (`initialize` with
  `capabilities.experimental.zornov.frame_paths: true`, or
  `ZORNOV_FRAMES=paths`) gets each picture as a JPEG path in a text block
  instead of base64.

Not done yet:

- The same three things through a window (`zornov mcp --attach`). There,
  calls run in the window's queue, which moves long calls to background jobs
  of its own, so export progress has to come from those jobs instead of
  polling `export_status`.
- Truly concurrent calls during an export. The stdio server still answers
  one message at a time. With progress, the agent sees the export move, but
  it cannot make other calls until the export ends.
- Progress for other long jobs (transcode, generate, see), which report
  through `job`.
- Change notices for a single clip (`studio://project/clip/{id}`).

## Eval metrics

`zornov eval` records each task's calls (`DIR/TASK/agent-trace/TASK.jsonl`)
and adds a `friction` object to each task in `results.json` and each line of
`history.jsonl`:

| metric | meaning |
|---|---|
| `calls` | tool calls the agent made |
| `calls_to_goal` | calls up to the last one that changed the project |
| `errors` | calls that failed |
| `retry_rate` | retries after an error, per call |
| `silent_drop_rate` | edits with a silent drop, per edit |
| `undo_rate` | changes undone right after, per change |
| `repeated_reads`, `feedback` | counts |
| `reply_tokens` | what the replies cost |
| `wall_seconds` | from the first call to the end of the last |

| `speed.record_us` | mean, p99 and max of what recording cost a call |
| `speed.stream_queue_us` | the same, for the wait to reach the writer and the live tap |
| `speed.fix_to_retry_s` | from `zornov dev swap`: build, load and retried call |

Each `history.jsonl` line also has a `speed` summary over all tasks
(`record_us_mean`, `stream_queue_us_mean`, `fix_to_retry_s_mean`).

Each run prints a friction line under the score, so a commit is measured on
what agents go through as well as on the result. `zornov trace report
DIR` gives the details for the same run.
