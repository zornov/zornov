# Automation security

Anyone can write issues, comments and pull requests, so any automation that
reads them is reading text a stranger may have written to steer it (prompt
injection). Studio keeps this safe in two ways:

- **Limits:** automation only ever proposes (labels, comments, draft pull
  requests); people merge and release. Jobs that read outside text hold no
  secrets and no write access to code.
- **Alarms:** the checks below catch the common tricks and stop automation
  until a person looks.

## What runs

| Check | Where | What it does |
|---|---|---|
| Text check | `intake` workflow, on every new or edited issue, comment and pull request | Looks for hidden text (invisible Unicode, text-direction tricks, HTML comments and styling that hide text), text written to steer an AI, smuggled data and canary values. A hit adds the `needs-human` label. |
| Pull request risk | `intake` workflow | Labels changes to AI instruction files (`risk: agent-files`), workflows and checks (`risk: ci`), build scripts and procedural macros (`risk: build-script`), dependencies (`risk: dependencies`), binaries (`risk: binary`) and hidden or AI-directed text in code (`risk: hidden-text`). |
| Kill switch | `intake` workflow | A canary hit, or more than `KILL_SWITCH_LIMIT` (default 5) flagged items in an hour, sets the repository variable `AGENTS_ENABLED` to `false`. |
| Workflow lint | `build` workflow | `zizmor` (injection, risky triggers, broad tokens, credential leaks, cache poisoning) and `actionlint` on every workflow file. |
| AI credit | `build` workflow | Fails when a commit credits an AI assistant. |
| Code owners | `.github/CODEOWNERS` | Workflows, checks, build scripts, dependencies and AI instruction files need a maintainer's review. |

The checker is `crates/dev/guard` (`studio-guard`). It has no dependencies,
builds in seconds, and its tests hold known injection shapes and ordinary
issues so the rules cannot quietly stop working:

```bash
cargo test -p studio-guard
cargo run -q -p studio-guard -- scan some-issue.txt
cargo run -q -p studio-guard -- pr-risk origin/main HEAD
```

`scan` exits 0 (nothing, or notes only), 1 (a person should look) or 2 (a
canary appeared).

## One-time setup on GitHub

1. **Variable** `AGENTS_ENABLED` = `true` (Settings > Secrets and variables >
   Actions > Variables). Optional: `KILL_SWITCH_LIMIT`.
2. **Secret** `STUDIO_GUARD_CANARIES`: one or more made-up values that look
   like secrets (for example `sk-canary-` followed by random characters),
   comma separated. Put the same values in the environment of every
   automation job. They have no power; if one ever appears in a comment or
   pull request, something leaked, and automation stops.
3. **Secret** `KILL_SWITCH_TOKEN`: a fine-grained personal access token for
   this repository only, with **Variables: read and write** and nothing else.
   Only the kill-switch job sees it, and that job never sees outside text.
4. **Branch protection** on `main`:
   - require a pull request with one approval, and **Require review from Code
     Owners**;
   - require the status checks `build`, `credit`, `workflows`, `deny` and
     `assets`;
   - **Do not allow bypassing the above settings**;
   - block force pushes and deletion.
5. **Code security**: turn on secret scanning with push protection, and
   Dependabot alerts.
6. **Actions settings**: workflow permissions "Read repository contents";
   "Require approval for all outside collaborators" for fork pull requests.

## Rules for any automation added later

Every workflow that runs an AI agent or other automation on outside input
must:

- start only when `vars.AGENTS_ENABLED == 'true'`, and skip any issue or pull
  request labelled `needs-human`;
- be started by a maintainer (a label only maintainers can add, or a command
  whose author is checked to be a collaborator), never by a stranger's text
  alone;
- read its instructions and configuration from the base branch, never from
  the pull request it is looking at;
- never build or run code from a pull request in a job that holds secrets;
- have the minimum `permissions:`, and no secrets beyond its own short-lived
  token and the canary values;
- pass everything it is about to post through
  `studio-guard scan --outgoing` (with `STUDIO_GUARD_CANARIES` set) and post
  nothing if that exits non-zero;
- open draft pull requests only, never merge, release or touch `main`.

The start of such a job:

```yaml
jobs:
  agent:
    if: vars.AGENTS_ENABLED == 'true' && !contains(github.event.issue.labels.*.name, 'needs-human')
    permissions:
      contents: read
      issues: write
    env:
      STUDIO_GUARD_CANARIES: ${{ secrets.STUDIO_GUARD_CANARIES }}
```

## When automation stops

1. Open the issues and pull requests labelled `needs-human` and read them as
   rendered **and** as raw text (hidden text only shows in the raw view).
2. If one is an attack, lock or delete it; if a canary appeared, rotate the
   canary values and check what the automation could reach.
3. Set `AGENTS_ENABLED` back to `true`.
