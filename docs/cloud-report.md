# Cloud report: security holes and readback

Work done in the cloud on four package branches. Each package was branched from `main` at `b60195f`, verified, and merged with `--no-ff` into `cloud/integration`. No package touches the areas being changed locally. The one exception is `app/src/agent/web.rs`, which was agreed in advance.

| Package | Branch head | Merge on `cloud/integration` |
|---|---|---|
| Web fetch guard | `cloud/sec-webfetch` `687d35e` | `5e99185` |
| OFX worker launch | `cloud/sec-ofx` `130dcdc` | `ece9f06` |
| Updater checks | `cloud/sec-update` `41a4491` | `7bb1aa0` |
| Readback ring and pooling | `cloud/perf-readback` `10527fa` | `ab05968` |

There are no `Cargo.lock` changes and no dependency changes. The only manifest edit is a `[[bench]]` entry in `crates/picture/render/Cargo.toml`.

## How it was verified

- CI pins Rust 1.99.1, so all checks ran on 1.99.1. `studio-plugins` does not build on 1.97 because `ofx/host.rs` uses C-variadic functions.
- The GPU tests ran on Mesa lavapipe, a software Vulkan driver, so they actually ran rather than skipping for lack of an adapter.
- Tests that talk to a local HTTP server need the sandbox's `HTTPS_PROXY` variables cleared. `studio-update`'s `tests/e2e.rs` and `studio-net`'s localhost tests fail on `main` too when those variables are set, and pass once they are cleared.
- For every package I read the diff and reran its tests myself before merging.
- I then ran `cargo test --workspace --no-fail-fast` once on the merged `cloud/integration` (Rust 1.99.1, lavapipe, proxy variables cleared, under `timeout 7000`). Across 213 test binaries: **4044 passed, 0 failed, 62 ignored**.

## 1. Web fetch guard (`cloud/sec-webfetch`)

**What was wrong.** `refuse_private` (`app/src/agent/web.rs`, formerly line 99) only matched the URL's host as text. The fetch went through `studio_net::get`, which follows up to 8 redirects on its own. That left three ways past the guard:
- **Redirects:** a public page could 30x-redirect to `http://127.0.0.1/` or `http://169.254.169.254/`, and the redirect was followed without any check.
- **DNS:** a public name that resolves to `127.0.0.1` or `169.254.169.254` passed the check.
- **Other ways of writing an address:** `127.1`, `0x7f.0.0.1`, `2130706433`, `0177.0.0.1` and `[::ffff:127.0.0.1]` all passed.

**The fix.**
- **Every resolved address is checked.** `blocked_ip` covers loopback, unspecified, private, link-local, CGNAT `100.64/10`, multicast and broadcast, `0/8`, and the documentation and benchmarking ranges. On IPv6 it covers ULA `fc00::/7`, `fe80::/10`, `fec0::/10`, Teredo and discard-only addresses. Addresses that carry an IPv4 address inside them (IPv4-mapped, IPv4-compatible, NAT64 `64:ff9b::/96`, 6to4) are judged by that IPv4 address.
- **All address forms are parsed.** `host_ip` reads every IPv4 form browsers accept: 1 to 4 parts, each decimal, `0x` hex or leading-`0` octal.
- **A backslash ends the host**, the same way the `url` crate reads http(s) links. Without that, `http://127.0.0.1\@evil.com` would be checked as `evil.com`.
- **One bad address refuses the name.** `resolve_public` resolves the name itself and refuses it if any of its addresses is blocked, rather than dropping that address and using the others, so rebinding tricks have nothing to choose from.
- **Redirects are followed by hand.** `fetch_guarded` follows up to 8 redirects itself. Every hop, including the first, re-runs the scheme check, `refuse_private`, the DNS lookup and the address check.
- **The connection goes to the checked address.** The fetch then connects to exactly the addresses that were checked, through the new `studio_net::get_pinned`. That function uses a ureq agent with `redirects(0)` and a resolver that returns only the given addresses. TLS and the `Host` header still use the name.
- **No API change.** `refuse_private` keeps its signature (`agent/mod.rs:2715` calls it).

**Tests** in `web.rs` `mod tests`, run with `cargo test -p studio-app --bins agent::web`:

| Test | Before | After |
|---|---|---|
| `a_redirect_to_a_local_address_is_not_followed` (real local server returning 302) | fail | pass |
| `a_redirect_to_a_name_that_resolves_locally_is_not_followed` | fail | pass |
| `names_that_resolve_to_local_addresses_are_refused` | fail | pass |
| `odd_ip_literals_are_refused` | fail | pass |
| `blocked_ip_edges` | fail | pass |
| `a_backslash_ends_the_authority_like_a_slash` | fail | pass |
| `studio-net`: `pinned_get_connects_to_the_given_address_and_leaves_redirects` | (new) | pass |

All 11 `agent::web` tests pass, and all 5 `studio-net` tests pass.

**Limit.** When `HTTPS_PROXY` or `ALL_PROXY` is set, the proxy makes the connection, so it cannot be pinned to the checked address. The per-hop address check still runs, but it can only advise. A user-configured proxy is a trust choice the user has made.

**Files:** `app/src/agent/web.rs`, `crates/project/net/src/lib.rs`. The net change only adds `Hop` and `get_pinned`; existing functions are unchanged.

## 2. OFX worker launch (`cloud/sec-ofx`)

**What was wrong.** In every build, `STUDIO_OFX_WORKER` chose the program launched for each OFX plugin (`crates/plugins/plugins/src/ofx/mod.rs:121-127`). When `current_exe()` failed, the code fell back to the bare name `zornov`, which is looked up on `PATH`.

**The fix.**
- **Choice logic.** `worker_program(env, exe, allow_env)` now picks the program, and `worker_command` calls it with `allow_env = cfg!(debug_assertions) || cfg!(test)`.
- **Release builds ignore the variable.** In release, `STUDIO_OFX_WORKER` has no effect. The program launched is `studio-ofx-worker` next to the current executable if that file checks out, and the executable itself otherwise.
- **What the bundled worker must pass.** It must be a regular file (checked with `symlink_metadata`, so a symlink is refused), its canonical folder must be the executable's canonical folder, and on unix it must not be writable by group or others.
- **No `PATH` lookup.** An executable that can't be resolved now gives a load error.
- **No public API change.** `worker_command` now returns `Result`, but it is private.

**Tests** in `ofx::worker_program_tests`, run with `cargo test -p studio-plugins --lib ofx`. All 7 pass after the fix.
- `env_honoured_when_allowed`
- `env_ignored_when_not_allowed`
- `sibling_chosen_when_valid`
- `falls_back_to_exe`
- `unresolvable_exe_is_an_error_not_a_path_lookup`
- `symlinked_sibling_rejected` (unix only)
- `world_writable_sibling_rejected` (unix only)

Against the original logic, 4 of the 7 failed: `env_ignored_when_not_allowed`, `symlinked_sibling_rejected`, `world_writable_sibling_rejected` and `unresolvable_exe_is_an_error_not_a_path_lookup`.

**Files:** `crates/plugins/plugins/src/ofx/mod.rs`.

## 3. Updater checks (`cloud/sec-update`)

**What was wrong.**
- **Unchecked folder swap.** For a folder install, `apply_pending` (`apply.rs:285`) swapped in the staged, unpacked folder without checking its bytes again. Only the signature over the entry was re-verified. The signed hash covers the archive, and `state.rs` deletes the archive after unpacking. So anyone able to write the updates folder between staging and the next start could replace what got installed.
- **Env overrides in every build.** `STUDIO_UPDATE_URL` and `STUDIO_UPDATE_PUBKEY` (`config.rs:42`) applied in every build, so whoever could set the environment could serve a self-signed "update".

**The fix.**
- **The archive is kept.** `stage()` keeps a copy of the archive beside the staged folder (`kept_archive`: `staged` becomes `staged.tar.gz`). The copy outlives `state.rs` deleting the original.
- **It is re-checked right before the swap.** `restage()` reads the kept copy into memory once, capped at the signed size. It checks size and SHA-256 against the signed entry, unpacks fresh from those same bytes into the staged folder, and re-checks the manifest. Only then does the swap run.
- **Why from memory.** Hashing and unpacking from the same bytes means the file can't change between the check and the use.
- **Failures abort.** A missing or mismatched archive stops the update through `fail()`.
- **AppImage and MSI.** These already re-hash the file just before replacing or running it (`check_file`). That is kept, and a regression test now covers it.
- **Env overrides gated.** `load()` now calls `load_from(env, cfg!(debug_assertions) || cfg!(test))`. `STUDIO_UPDATE_URL` and `STUDIO_UPDATE_PUBKEY` apply only in debug and test builds. `STUDIO_CRASH_ENDPOINT` is unchanged, since it can't lead to running code.

**Tests**, run with `cargo test -p studio-update` (23 unit tests and the e2e test pass; the e2e runs the new re-stage path end to end):

| Test | Before | After |
|---|---|---|
| `a_tampered_staged_folder_is_replaced_by_the_signed_archive` | fail (installed `"evil"`) | pass |
| `a_tampered_or_missing_kept_archive_aborts_the_swap` | fail | pass |
| `release_builds_ignore_the_feed_and_key_overrides` | fail (took `https://evil/feed.json`) | pass |
| `a_tampered_staged_appimage_is_refused_and_the_target_kept` | pass (regression guard) | pass |

**Notes.**
- **Updates already waiting.** An update staged before this change has no kept archive, so it is refused once at the next start and downloaded again.
- **Leftover temp file.** `crates/dev/xtask/src/repro.rs:212` calls `apply::stage` in a test, which now also leaves `<out>.tar.gz` in the temp folder. It's harmless. I didn't touch it because xtask is local-only.

**Files:** `crates/project/update/src/apply.rs`, `crates/project/update/src/config.rs`. `state.rs` is untouched.

## 4. Readback ring and pooling (`cloud/perf-readback`)

**What was wrong.**
- **Blocking readback.** Every frame read back by `GpuCompositor::read_bytes` (`gpu/mod.rs` around line 740) and by `Renderer::run` (`realtime/draw.rs` 192-218) created a new staging buffer, then waited for the GPU with `PollType::wait_indefinitely`.
- **Unpooled buffers.** The preview buffers were created per call.

**The fix.**
- **New module.** `realtime/readback.rs` adds `Readback`, which holds pooled staging buffers.
- **The exact path pools its buffer.** `sync_target` / `read_sync` reuse one `MAP_READ` buffer per size, and rows are unpadded into a reused `Vec`. `render_srgb`, `render_linear` and every `GpuCompositor` readback use it. They still wait and still return the exact frame asked for, which export and stills require.
- **The preview path uses a ring.** `ring_target` / `ring_latest` keep three staging buffers:
  - Frame N is copied into slot N%3 and `map_async` starts on it.
  - A non-blocking `PollType::Poll` then lets it hand out the newest finished frame as RGBA8, normally N-1.
  - It waits only on the first frame after a size change, so there is always a picture, or when the GPU falls more than 3 frames behind.
- **New method.** The ring is reached through the new `Renderer::render_srgb_pipelined(w, v, o, &mut Vec<u8>) -> (Latest, Stats)`. `Latest` reports `lag` and `fresh`.
- **Counters.** A `#[doc(hidden)] readback_stats()` exposes wait, poll and buffer-creation counts for the tests and the bench.
- **Public signatures unchanged.** Every existing public signature is the same.

**Tests**, in the new `crates/picture/render/tests/readback.rs`. These cover new code, so there was no failing "before".
- **Golden pixels:** gradient frames at 67 px wide (exercises row padding) and 64 px, checked pixel for pixel over 8 frames. Frame 0 has lag 0; after that, frame N shows frame N-1 with lag exactly 1.
- **Exact path:** returns frame N and uses one buffer.
- **No blocking:** the ring does no blocking wait after the first frame over 30 frames and creates only 3 buffers. A resize rebuilds the ring with one wait.
- **Matches `render_srgb`:** `render_srgb_pipelined` gives the same picture as `render_srgb` for a still camera.

The whole `studio-render` and `studio-gpu-device` suites pass, including `realtime` (10), `golden` (8), `readback` (5), `agreement`, `comp` and `reuse`.

**Benchmark.** Run with `cargo bench -p studio-render --bench readback -- --frames 30` on llvmpipe, the software GPU, so these numbers show the stall being removed rather than real-hardware speed. The world render is dominated by the software rasteriser.

| Path | Case | Median ms/call | Blocking waits |
|---|---|---:|---:|
| blocking (before) | 1080p RGBA8 upload + readback | 2.31 | 30 |
| ring (after) | same, 30/30 frames fresh | 0.93 | 1 |
| blocking (before) | world 640x360 `render_srgb` | 177.2 | 20 |
| ring (after) | world 640x360 `render_srgb_pipelined` | 173.1 | 1 |

**Files:**
- `crates/picture/render/src/realtime/readback.rs` (new)
- `crates/picture/render/src/realtime/draw.rs`
- `crates/picture/render/src/realtime/mod.rs`
- `crates/picture/render/src/gpu/mod.rs`
- `crates/picture/render/tests/readback.rs` (new)
- `crates/picture/render/benches/readback.rs` (new)
- `crates/picture/render/Cargo.toml` (the `[[bench]]` entry only)
- `crates/picture/gpu-device` is untouched.

## Exact files touched (whole branch against `main`)

```
app/src/agent/web.rs
crates/project/net/src/lib.rs
crates/plugins/plugins/src/ofx/mod.rs
crates/project/update/src/apply.rs
crates/project/update/src/config.rs
crates/picture/render/Cargo.toml
crates/picture/render/benches/readback.rs
crates/picture/render/src/gpu/mod.rs
crates/picture/render/src/realtime/draw.rs
crates/picture/render/src/realtime/mod.rs
crates/picture/render/src/realtime/readback.rs
crates/picture/render/tests/readback.rs
```

## Left undone

- **The editor doesn't use the ring yet.** The preview still calls the blocking `render_srgb`, so it gets no speedup until `app/src/editor/viewport/world.rs:584` switches to `render_srgb_pipelined`. That file belongs to the local UX team. When switching:
  - keep a persistent `Vec<u8>` for moving-camera previews
  - check `fresh` and `lag` before using the result
  - keep `render_srgb` for stills and the final refined frame
- **`GpuCompositor` readbacks still wait.** They are pooled but stay exact and blocking, because their callers (compositing, export) need the exact frame. Pipelining them needs callers that can take one frame of lag.
- **`upload_frame` staging in `gpu/mod.rs` is not pooled.** Reusing that buffer could overwrite it while an earlier recorded copy is still reading from it.
- **Not checked in this sandbox:**
  - a real hardware GPU benchmark (only llvmpipe was available)
  - a release build of `studio-plugins`
- **`download` doesn't use the new guard yet.** `app/src/agent/download.rs` still fetches through `studio_net::download`, which follows redirects and doesn't pin the address. It should use the same per-hop check and address pinning as `web.rs`. That file is local-only.
- **`OFX_PLUGIN_PATH`** (`ofx/mod.rs:50`) still lets the environment add folders where plugin bundles are searched for. It is not a program path, but it does decide what code is loaded into a worker. Consider applying the same debug-only rule.
- **`crates/plugins/plugins/src/author.rs:57`** has another `Command::new(program)`. It was not reviewed.

## Needs from local

1. **Render-node pairing token.**
   - **Weak token:** `wire::new_token` (`crates/media/deliver/src/wire.rs:159`) hashes `nonce()` (`wire.rs:131-143`), which is built from the time in nanoseconds, the process ID, a counter and a stack address. None of these come from a CSPRNG, and an attacker can guess most of them.
   - **Stored in plaintext:** `app/src/deliver_run.rs:886-889` keeps it in `<work>/token`.
   - **Exposed:** the node listens on `0.0.0.0` by default (`deliver_run.rs:866`) and prints the token.
   - **Fix:**
     - draw the token from the OS RNG (`ring::rand::SystemRandom` is already in the workspace)
     - save the file with `0600` permissions
     - default to listening on loopback, or require `--listen` before exposing the node on the LAN
2. **`download` reads any local file.**
   - **The hole:** `app/src/agent/download.rs:101` (`resolve_local`) accepts `file://` links, `~/...` and relative paths. The agent can therefore bring any readable file on the machine into the project, for example `~/.ssh/id_ed25519`.
   - **Fix:** limit local sources to folders the user has picked, or the project folder, and ask before reading anything outside them.
   - **Also:** move `download`'s network path onto the guarded, pinned fetch described in section 1.
3. **Plugins without executable memory.** `crates/plugins/plugins/src/runtime.rs` (`Config::new()` at line 48) runs WASM plugins with Cranelift JIT. Switching to wasmtime's Pulley interpreter would mean plugins need no executable memory at all. That matters for hardened runtimes such as macOS without the JIT entitlement, and it shrinks the attack surface. `runtime.rs` is being changed locally, so this is left to the local team.
