# Contributing to Zornov

Thank you for helping. Zornov is licensed under the GNU Affero General Public
License v3.0 or later (see `LICENSE` and `NOTICE`), and is also offered under
a commercial licence by Ashrox LLC (see `LICENSING.md`).

## Licence of contributions

By submitting a contribution (code, documentation, assets or anything else)
to this project, you agree that:

1. You wrote it, or otherwise have the right to submit it under these terms.
2. It is licensed under the GNU Affero General Public License v3.0 or later.
3. You also grant Ashrox LLC a perpetual, worldwide, non-exclusive,
   royalty-free, irrevocable licence to use, modify, sublicense and
   distribute your contribution under any licence terms, including terms
   other than the GNU Affero General Public License, including Zornov's
   commercial licence.

You keep the copyright in your contribution.

## Building on Linux in a container

Zornov builds natively on macOS, Windows and Linux with the Rust toolchain.
To build and test the Linux version without installing anything else, use
the container in `docker/Dockerfile`, which CI uses too:

- **VS Code or GitHub Codespaces:** open the repository and choose *Reopen in
  Container* (`.devcontainer/`).
- **Any terminal with Docker:**

  ```
  docker build -t zornov-linux docker
  docker run --rm -it -v "$PWD:/work" zornov-linux cargo test --workspace
  ```

The container has a software graphics driver, so the GPU tests run without a
graphics card, and it keeps its build output apart from the checkout's
`target/` folder. macOS and Windows builds run natively (containers only run
Linux).

To check that a change still compiles for Windows without a Windows machine
(it catches code written for one platform and not marked as such), run in
the container:

```
docker run --rm -v "$PWD:/work" zornov-linux cargo check --target x86_64-pc-windows-gnu --workspace --all-targets
```

It compiles but does not run anything; running and testing Windows needs
Windows itself. See [`docs/windows.md`](docs/windows.md) for building there,
and for the two programs Windows ships (`zornov.exe` for the window,
`zornov-cli.exe` for the terminal).

## Git hooks

Turn on the project's checks once after cloning:

```
git config core.hooksPath .githooks
```

Before each commit they stop: leftover conflict markers, anything that looks
like a private key or live API key, a new source file without the copyright
header, and a picture, video, sound, font, plugin or fixture that does not
match `assets.lock` (after adding or changing one on purpose, run
`cargo xtask assets-lock` and commit `assets.lock` with it). CI runs the same
checks, so `git commit --no-verify` only postpones them.

## Copyright headers

Every source file made for Zornov starts with:

```
// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
```

`cargo xtask headers` adds it to files that lack it, and
`cargo xtask headers --check` lists them. Code taken from another project
keeps that project's own notice instead (see `NOTICE`).

## Contributor Licence Agreement

Before your first contribution is merged, you sign Ashrox LLC's Contributor
Licence Agreement, once. It sets out the same grant as the section above in a
signed form. On your first pull request, CLA Assistant comments with a link;
you sign in with GitHub and agree, and the check turns green. The agreement
covers the GitHub account your commits' email address belongs to, so make
sure that address is on your account. Contributions made as part of your job
may need your employer's agreement too; please check before you submit.

## Sign-off

Each commit carries a `Signed-off-by:` line (`git commit -s`) with your real
name or the name you are known by, certifying the
[Developer Certificate of Origin 1.1](https://developercertificate.org/):
that you wrote the change or otherwise have the right to submit it under
these terms. The `dco` check on pull requests looks for a sign-off from each
commit's author (same email address). To sign off commits already made:
`git rebase --signoff main`, then push with `--force-with-lease`.

## Code, models and content from elsewhere

- Only add third-party code, fonts, pictures, sounds, music, models or test
  files under permissive licences (MIT, Apache-2.0, BSD, Zlib, CC0 and
  similar), so Zornov can be offered under both the AGPL and its commercial
  licence. No copyleft (GPL, LGPL, AGPL, MPL) or non-commercial material from
  others; `cargo deny check` enforces this for crates.
- Say where it came from, its exact revision or version, its copyright
  holder and its licence, and add it to `NOTICE` (code) or `ATTRIBUTIONS.md`
  (library content). Keep the original notice at the top of copied files.
- Do not copy code, artwork or text from other products, from decompiled or
  leaked sources, or from anything you obtained under a confidentiality or
  employment agreement.
- If an AI tool wrote a substantial part of a contribution, say so in the
  pull request. You are still responsible for having the right to submit it.

## Security problems

Do not report security problems in public issues or pull requests; see
`SECURITY.md`.

## Decisions

How the project is run, and who decides on releases, licensing and the
Zornov name, is described in `GOVERNANCE.md`.

## Interface text

Every word the interface shows is translated (nine languages). When you add
or change UI text:

1. Write the English in the code as usual (`Button::primary("Export")`).
   Shared widgets translate what they are given; text drawn straight with
   egui's painter or `RichText` goes through `crate::i18n::shown("…")`;
   text built with `format!` is wrapped in `crate::i18n::tf(...)`; counts
   and plurals use a message with `tr_args` (see `docs/localisation.md`).
2. Run `cargo run -p studio-i18n --bin i18n-scan -- --add`. It adds the new
   text to `crates/project/i18n/locales/en.ftl` and lists it in
   `locales/pending.txt`, so other languages show the English until
   translated.
3. `cargo test -p studio-i18n` fails if an interface string has no message.

New controls need an accessible name (the shared widgets give one; for a
custom-painted control call `response.widget_info(...)`) and must work from the
keyboard. `docs/accessibility.md` lists the rules; the headless tests in
`app/src/ui_tests.rs` check names, keyboard navigation and layout.
