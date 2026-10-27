# Seeing what agents go through

Agents drive Zornov through its tools (MCP), and until now the people
working on Zornov only found out what went wrong by reading transcripts
afterwards. These tools close that loop: every call is recorded, the
friction in the records is found and ranked for you, any recorded call can
be run again on the current build, and a coding agent in this repository can
watch an agent inside the app as it works.

All of it is local. Nothing is sent anywhere.

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
| `module` | the Rust module that answered (`agent::quality`, `agent::mod (edit: set, animate)`) |

Lines go to `.zornov/agent-trace/<session>.jsonl` in the project folder, or
`agent-trace/` in Zornov's cache when there is no project. A session is one
process (`<start time>-<pid>`); `zornov eval` starts one for each task.

**Redaction.** Strings under keys such as `data`, `bytes`, `content`,
`source`, `pixel`, `wgsl`, `token`, `api_key` and `password`, `data:` URLs,
long base64-looking strings and any string over 4 000 bytes are replaced by
`[redacted N bytes]`. Paths, ids, times and the words of titles stay, so a
call can be replayed. Pictures in replies are counted, never kept.

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
reply. It goes into the trace like any call, and the report lists it. An
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
when the session began (`versions` keeps snapshots). Calls that reach
outside the project or cost money (download, generate, ai, read,
search_media, share, export, deliver, plugins and similar) are skipped
unless you pass `--all`, and so is any call whose arguments were redacted.

**`--emit-test`** writes every call that failed in the recording as a JSON
fixture in `app/tests/agent_replays/`: the earlier calls that changed the
project (the setup), the call, and what it does on this build from a blank
project (`expect`). The test `recorded_agent_calls_still_behave` in
`app/src/agent/replay.rs` runs every fixture with `cargo test`. The fixture
records current behaviour. When you fix the tool, change `expect` to what it
should now do, and the test keeps it that way.

## The live tap

```bash
zornov mcp --observe --text          # the window opened last
zornov mcp --observe --project my-project
zornov mcp --observe /path/to/studio-agent-123.sock
```

This follows an open window's calls as they happen, whatever makes them: the
AI panel, Claude Code attached to the window, or `zornov agent`. Each call is
one trace line on stdout, with `module` naming the Rust module that answered
it. `--text` prints one readable line per call instead, plus the error, or
the expected and got of a `feedback` call. It works whether or not the
recorder is writing to disk.

It goes through the window's agent socket, which only your user can open
(on Windows, the token file in your temp folder). After the first line,
`{"observe": true}`, the window only writes to that connection and never
reads it, so an observer cannot run a tool or change the project. A coding
agent in this repository can run the command in the background (Claude
Code's Monitor tool, or `| jq` in a terminal) and see what an agent inside
the app is doing, then fix the module the trouble comes from.

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

Each run prints a friction line under the score, so a commit is measured on
what agents go through as well as on the result. `zornov trace report
DIR` gives the details for the same run.
