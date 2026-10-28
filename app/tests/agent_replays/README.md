# Recorded agent calls

Each `.json` file here is a call an agent made that failed, written by
`zornov replay <trace> --emit-test` (see `docs/agent-observability.md`).
It holds the calls before it that changed the project (`setup`), the call,
and what the call does on the build that wrote it (`expect`).

`cargo test recorded_agent_calls_still_behave` runs every file. When you fix
the tool behind one, change its `expect` to what the call should now do.
