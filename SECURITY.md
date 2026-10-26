# Security policy

Zornov is maintained by Ashrox LLC. We take reports about its security
seriously and are grateful to the people who make them.

## Reporting a vulnerability

**Please do not open a public issue, discussion or pull request for a
security problem.**

Report it privately through GitHub: open the repository's **Security** tab
and choose **Report a vulnerability**. Only the maintainers can see the
report, and we can discuss and fix it there before anything is public.

Please include:

- the Zornov version and operating system;
- what an attacker can do, and what they need first (a crafted project or
  media file, a network position, local access, and so on);
- steps or a file that reproduce it, if you have them.

Please keep the details private until a fix is released or until we agree a
date with you.

## What to expect

- We confirm we have your report within **5 working days**.
- We tell you whether we consider it a vulnerability, and our plan, within
  **15 working days**.
- We aim to release a fix within **90 days** of the report, sooner for
  serious problems, and keep you informed along the way.
- When the fix is out we publish an advisory and, if you wish, credit you by
  the name you choose.

We do not currently offer payment for reports.

## Supported versions

Security fixes go into the latest release, which reaches people through
Zornov's signed updates. Older releases are not patched; please update.

## Scope

In scope: the Zornov application, its command-line tools, its file formats
and media decoders (a crafted project, image, video, audio or font file that
crashes Zornov, reads or writes files it should not, or runs code), the
update mechanism and its signatures, and the build and release tooling in
this repository.

Out of scope: problems in third-party services Zornov can connect to
(report those to the service), attacks that need an already-compromised
computer, and reports from automated scanners without a demonstrated impact.

## Good-faith research

We will not pursue or support legal action against anyone who, in good
faith, researches and reports a vulnerability in line with this policy:
testing only against their own installation and data, not accessing or
changing other people's data, not disrupting services, and giving us a
reasonable time to fix the problem before disclosing it.

## Verifying downloads

Release builds are signed: the macOS app with an Apple Developer ID and
notarized, and every update archive with Zornov's Ed25519 update key, which
the app checks before installing anything. Download Zornov only from the
project's official release page.

Issues, comments and pull requests are checked for hidden text and prompt
injection before any automation reads them; see
[docs/automation-security.md](docs/automation-security.md).
