# Accounts, credits, the cloud and the marketplace

**Off by default.** A normal Studio build has none of this: no Account page
in Settings, no sign-in, no plans, licences or credits, no marketplace, no
cloud render, sync or review links, no `account` tool for the AI, and
nothing about any of it in the guide or the interface. The whole editor
works the same for everyone. The code is kept, behind one build switch,
the cargo feature `commerce` of `studio-app`.

To build with it:

```bash
cargo build --release -p studio-app --features commerce
cargo run -p studio-app --features commerce
```

The feature also adds the Settings messages in `crates/project/i18n/locales/commerce/`
(`studio-i18n/commerce`), and the account section of the guide
(`guide_commerce.md`, `tool_reference_commerce.md` in `app/src/agent`).
Without the feature none of those files is compiled in. The default build is
checked for this by tests (`agent/commerce_off.rs`, the `studio-i18n`
catalogue test); `cargo test -p studio-app --features commerce` runs the
account tool and page tests, and `cargo test -p studio-entitlements -p
studio-commerce -p studio-cloud-server` the crates behind it.

What follows describes the build with `commerce` on. Nothing on this page
is sent anywhere until you connect a server.

## The pieces

| crate | what it is |
|-------|-----------|
| `studio-entitlements` | Plans as data (`plans.json`), feature flags, Ed25519-signed licence files that verify offline. |
| `studio-commerce` | The account client: sign-in, credits (prices, ledger, budget, quotes), cloud renders, review links, project sync, the marketplace, Stripe Checkout in test mode. |
| `studio-cloud-server` | A server anyone can run: accounts, licences, metered credits and an AI proxy, renders, review links, sync, the marketplace, payments. |
| `studio-app` feature `commerce` | The `account` agent tool, Settings ▸ Account and the running cost in the AI panel. Off by default; without it none of it exists in the app. |

The editor itself is never gated. Feature flags name only hosted or paid
extras: `ai.hosted`, `cloud.render`, `cloud.sync`, `cloud.review`,
`cloud.storage`, `market.buy`, `market.sell`, `team.admin`, `team.sso`,
`ai.priority`, `support.priority`. `ai.byok` (your own keys) is on every plan.

## Licences

A licence file is JSON:

```json
{ "payload": "{\"schema\":1,\"id\":\"L-…\",\"plan\":\"pro\",…}", "signature": "<Ed25519, hex>" }
```

The signature covers `studio-licence/1\n` followed by the payload bytes
exactly, so the file is never re-serialised before checking. Studio
accepts licences from:

- keys built in at compile time or set at run time in
  `STUDIO_LICENCE_PUBKEYS` (comma-separated 64-hex-digit Ed25519 keys);
- keys the person chose to trust, in `trusted-keys.json` (connecting to
  your own server with "Trust licences this server signs").

Checking needs no network. An expired licence keeps working for its
`grace_days`, then Studio is on the free plan again (the editor keeps
working). An edited or forged file is refused and Studio says why.

A licence can add or remove features on top of its plan (`add`, `remove`)
and override the monthly credits, so new offers need no app update.

## Where things are kept

`~/Library/Application Support/Studio/account/` (macOS; `%LOCALAPPDATA%\Studio\account`,
`$XDG_STATE_HOME/studio/account` elsewhere):

```text
cloud.json         cloud on/off and the server address
me.json            who is signed in, plan and balance as last seen
licence.json       the signed licence
trusted-keys.json  licence issuers you chose to trust
plans.json         the server's plans (with prices)
prices.json        the server's credit price per task
usage.jsonl        the usage ledger
budget.json        your monthly credit budget
```

The session token is kept in the Keychain (`STUDIO_NO_KEYCHAIN=1` keeps it
in a private file instead, for headless runs).

## Credits

A server publishes a price table (`GET /v1/prices`):

```json
{
  "schema": 1,
  "credit_value_micros": 10000,
  "currency": "usd",
  "tasks": [
    { "task": "transcribe", "unit": "minute", "provider": "openai", "model": "whisper-1",
      "cost_micros": 6000, "credits": 1 }
  ]
}
```

`cost_micros` is what the provider charges per unit; `credits` is what a
unit costs in credits. A server refuses to start if any task's credits are
worth less than its provider cost plus `margin_bps`.

Before a hosted task the AI calls `account {"action": "quote", …}` and
tells you the cost, the balance after and any budget warning; it runs the
task only when you agree. The server holds the credits (`reserve`),
charges what was really used, never more than was held (`commit`), or
gives them back (`release`). With your own key a task costs no credits and
is never blocked by the plan or the budget.

## Running a server

```bash
cargo run --release -p studio-cloud-server -- serve --data ~/studio-cloud --addr 127.0.0.1:8787
```

Then in Studio: Settings ▸ Account ▸ Server `http://localhost:8787`, tick
"Trust licences this server signs", Connect, and create an account. To
serve other computers put it behind a TLS proxy (Caddy, nginx) and use the
`https://` address; Studio refuses plain `http://` to anything but this
computer.

`<data>/server.json` (every field optional):

```json
{
  "name": "Our studio cloud",
  "public_url": "https://cloud.example.com",
  "plans": "plans.json",
  "prices": "prices.json",
  "packs": [{ "id": "c500", "credits": 500, "price_minor": 500, "currency": "usd" }],
  "creator_bps": 7000,
  "margin_bps": 0,
  "licence_days": 35,
  "grace_days": 14,
  "render_command": ["studio-render-worker", "{project}", "{out}"],
  "upstreams": {
    "openai": { "base": "https://api.openai.com", "header": "Authorization", "prefix": "Bearer ", "key_env": "OPENAI_API_KEY" }
  },
  "signup": true,
  "max_upload_mb": 2048
}
```

- `plans`: the plan list with `prices` filled in (a plan without a price
  cannot be bought). Without it the shipped plans are used.
- `prices`: the credit price table; hosted AI is off without one.
- `creator_bps`: a marketplace creator's share of each sale (basis points).
- `render_command`: runs a cloud render; `{project}` is the project JSON,
  `{out}` the file to write, `{dir}` the job folder.
- `upstreams`: AI providers the server forwards metered calls to
  (`POST /v1/ai/<provider>/<path>` with `x-studio-task` and
  `x-studio-units`), with the server's own key from the environment.

Other commands: `studio-cloud-server key --data DIR` prints the licence
public key; `studio-cloud-server issue --data DIR --plan pro --holder NAME
[--email E] [--days N] [--seats N]` prints a signed licence file (for
education or enterprise licences given by hand).

The server keeps everything in its data folder: `state.json`, the signing
key `issuer.pk8` (keep it secret and backed up), uploads under `review/`,
`market/`, `render/` and `sync/`.

## Payments (Stripe test mode only)

Checkout uses Stripe Checkout with a **test** key; live keys (`sk_live_…`)
and live-mode replies are refused, so no real payment can be taken. The
server reads the key at start-up from `STUDIO_STRIPE_TEST_KEY` or, on
macOS, the Keychain item "Studio API keys" / `stripe-test-secret-key`.
After paying, the buyer lands on `/billing/done`, which reads the session
back from Stripe and fulfils the order once; Studio's "Check payment" does
the same. A webhook at `/v1/stripe/webhook` (secret in
`STUDIO_STRIPE_WEBHOOK_SECRET`) is also accepted; its signature is
checked and the session is read back from Stripe rather than trusted.
Test cards: `4242 4242 4242 4242`, any future date, any CVC.

## Marketplace

Listings are templates, plugins, LUTs, sounds and assets. A creator on a
plan with `market.sell` lists an item and uploads its file; buyers get it
free, or after a checkout. Installing checks the file's SHA-256 and puts
it in `~/Movies/Studio/Marketplace/<Templates|Plugins|LUTs|Sounds|Assets>/<id>/`
with a `listing.json` beside it. Each sale is split by `creator_bps`
(the creator's part rounded down; the two parts always add up), and
`GET /v1/creator/statement` totals a creator's sales per currency.

## Project sync and review links

A project's sync space (`/v1/sync/<project>/…`) offers the same calls as a
shared folder: head, revisions, an atomic publish that fails as stale when
someone published first, members with their id blocks, review notes and
the team. Review links (`/r/<token>`) play a rendered file in the browser
and take timed notes, which `account review_notes` reads back.

## The agent tool

`account` with `action`: `status`, `plans`, `quote`, `budget`, `usage`,
`receipts`, `packs`, `upgrade`, `buy_credits`, `confirm`, `connect`,
`disconnect`, `sign_up`, `sign_in`, `sign_out`, `refresh`, `licence`,
`render`, `render_status`, `render_fetch`, `review`, `review_notes`,
`market`, `buy`, `install`, `installed`, `sell`, `statement`. See
`guide {"topic": "account"}`.

## Tests

```bash
cargo test -p studio-entitlements -p studio-commerce -p studio-cloud-server
```

`crates/business/cloud-server/tests/end_to_end.rs` runs the client against the
server on localhost with a local stand-in for Stripe's test API. Against
Stripe's real test mode (needs a test key and the network):

```bash
STUDIO_STRIPE_TEST_KEY=sk_test_... cargo test -p studio-commerce -- --ignored
```
