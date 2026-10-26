# Releasing Zornov

Everything runs through `cargo xtask`. Nothing here is secret except what you
keep in your keychain or CI secrets; no command prints a key, password or
token, and none of them is committed.

| Command | Makes |
| --- | --- |
| `cargo xtask release-mac` | `dist/release/Zornov-<ver>-macos-<arch>.dmg` (Developer ID signed, hardened runtime, notarized, stapled) and a signed update archive |
| `cargo xtask package-msi` | `dist/release/Zornov-<ver>-windows-<arch>.msi` (needs WiX) |
| `cargo xtask package-windows` | `dist/zornov-windows-<arch>.zip` (portable) + signed update archive |
| `cargo xtask package-deb` | `dist/release/zornov_<ver>_<arch>.deb` |
| `cargo xtask package-appimage` | `dist/release/Zornov-<ver>-<arch>.AppImage` (needs `appimagetool`) |
| `cargo xtask package-linux` | `dist/zornov-linux-<arch>.tar.gz` (portable) + signed update archive |
| `cargo xtask update-key` | the Ed25519 update signing key, once |
| `cargo xtask feed` | `dist/release/feed.json`, the signed update feed |
| `cargo xtask licenses` | `dist/THIRD-PARTY-LICENSES.txt`, the licences of every crate built in |

The version is the workspace `version` in `Cargo.toml`. Packages are built with
`--locked`, build paths remapped and every archive date set to
`SOURCE_DATE_EPOCH` (default: the last commit's time), so the same commit gives
the same `.tar.gz`, `.zip` and `.deb`.

## Licence notices

Every package carries Zornov's `LICENSE`, `NOTICE` and `ATTRIBUTIONS.md`, the
licences of the fonts and icons in `app/assets`, the ACES licence
(`crates/picture/colour/LICENSE-ACES.md`) and `THIRD-PARTY-LICENSES.txt`: on macOS in
`Zornov.app/Contents/Resources/Licenses/`, in the `.deb` and AppImage in
`/usr/share/doc/zornov/`, and in the portable archives and the MSI next to the
program. The packaging commands make `THIRD-PARTY-LICENSES.txt` themselves
from `cargo metadata --locked` for every release target: each crate compiled
into Zornov (normal and build dependencies; Zornov's own crates are left out
unless vendored under another licence) with its licence expression and the
licence and notice files it ships, including those of fonts or C code it
bundles. A crate that ships none gets the standard text of its licence from
`crates/dev/xtask/licenses/` and its authors as the copyright holders; a licence
with no standard text there fails the build, so add the text when
`deny.toml` allows a new licence. Identical texts are given once. The file
depends only on the lock file, so it is the same on every machine.

## One-time setup: macOS signing and notarization

You need an Apple Developer Program membership.

1. **Developer ID Application certificate.** In Xcode (Settings ▸ Accounts ▸
   Manage Certificates) or at developer.apple.com, create a *Developer ID
   Application* certificate. It lands in your login keychain.
   Check: `security find-identity -v -p codesigning` lists it. With exactly one
   such identity `release-mac` finds it by itself; with several, set
   `STUDIO_SIGN_IDENTITY` to its name or SHA-1.
2. **Notarization sign-in**, either:
   - a keychain profile (simplest):
     `xcrun notarytool store-credentials studio-notary` (it asks for your Apple
     ID, team ID and an app-specific password from appleid.apple.com, and keeps
     them in the keychain). Then `export STUDIO_NOTARY_PROFILE=studio-notary`
     (add `STUDIO_NOTARY_KEYCHAIN=<path>` if you keep it outside the login
     keychain); or
   - an App Store Connect API key (best for CI): create a key with the
     *Developer* role in App Store Connect ▸ Users and Access ▸ Integrations,
     download the `.p8`, and set `STUDIO_NOTARY_KEY=<path to .p8>`,
     `STUDIO_NOTARY_KEY_ID=<key id>`, `STUDIO_NOTARY_ISSUER=<issuer id>`.
3. **Test the setup** (signs in to Apple, builds nothing):
   `cargo xtask release-mac --check`. Anything missing is listed together and
   the command stops before the long build.

`cargo xtask release-mac` then builds, assembles `Zornov.app`, signs it with
the hardened runtime and `packaging/macos/Zornov.entitlements`, notarizes and
staples it, builds the DMG, signs, notarizes and staples that too, and writes
the signed update archive. Options: `--no-notarize` (signed DMG, no Apple
round trip), `--identity ID`, `--exe PATH` (use a binary you already built).
Without credentials it fails with the list of what is missing.

The entitlements file asks for only the microphone (voice-overs). Zornov loads
no third-party libraries and generates no code at run time, so it needs
neither library-validation nor JIT exceptions; add one there only if a new
dependency requires it.

## One-time setup: updates

The app checks a feed at start and from Help ▸ Check for updates (by itself at
most once a day; Settings ▸ About turns that off), verifies its Ed25519
signature with the public key built into it, and shows a small notice in the
corner of the window: **Update now** downloads the build for this kind of
install and checks it, then **Restart now** installs it (or it installs at the
next start). **Later** keeps the notice away for a day.

1. `cargo xtask update-key` makes the private key
   (`~/Library/Application Support/Zornov/signing/update-key.hex` on macOS,
   mode 0600; a `Studio` folder left by the app's earlier name is moved to `Zornov` the first time `cargo xtask` or the app runs, key included) and prints the **public** key. Back the private key up: apps
   built with a public key accept only updates signed by its private half. In
   CI put the same hex in the secret `STUDIO_UPDATE_KEY`.
2. Edit `packaging/release.conf`: `public_key =` the printed key and
   `feed_url =` where you will host `feed.json`. For GitHub Releases that is
   `https://github.com/OWNER/REPO/releases/latest/download/feed.json`, and
   `cargo xtask feed` then points each archive at the `vX.Y.Z` release it was
   built for. Any static HTTPS host works too. The app also reads
   `STUDIO_UPDATE_URL` / `STUDIO_UPDATE_PUBKEY` at run time (for testing).
   With either empty the app never updates and Settings ▸ About says why.
3. After building the packages for a version, `cargo xtask feed --base-url
   https://downloads.example.com/studio --notes RELEASE_NOTES.md` writes
   `dist/release/feed.json`, checking every signature against the key. Upload
   `feed.json`, the `*.update.tar.gz` files, the `.msi` and the `.AppImage` in
   `dist/release/` (for GitHub, as assets of the `vX.Y.Z` release).

What the feed signs: for each platform the version, SHA-256 and size of its
update archive. The URL is not signed (the hash pins the bytes), so archives
can sit on any mirror. A feed signed with another key, a changed archive, a
different version or platform, or a version that is not newer is refused.

Each kind of install has its own feed key, so its own signature:

| Install | Feed key | Update |
| --- | --- | --- |
| macOS `.app` (from the DMG) | `macos-<arch>` | files swapped inside the bundle |
| Portable folder (`studio.install` beside the program, writable) | `windows-<arch>`, `linux-<arch>` | files swapped in the folder |
| Windows MSI (Program Files) | `windows-<arch>-msi` | the new `.msi` runs after Zornov quits, then starts it |
| Linux AppImage | `linux-<arch>-appimage` | the new file replaces the old one |

`package-msi` and `package-appimage` sign their output for the feed. A `.deb`
or a development build only learns that a newer version exists (with a GitHub
feed the notice links to the release page); a package manager should own
`.deb` installs.

Updating swaps files in place at the next start and puts the old ones back if
anything fails. **Restart now** (in the notice or Settings) does it right away.

## Windows

`cargo xtask package-msi` needs the WiX toolset v4 or later
(`dotnet tool install --global wix`). To sign, install your code-signing
certificate in the Windows certificate store and set
`STUDIO_WIN_SIGN_THUMBPRINT` to its SHA-1 thumbprint; `signtool` then signs the
MSI. (The `UpgradeCode` in the MSI is fixed, so a newer MSI upgrades the old
install.)

## Linux

`package-deb` needs nothing extra. `package-appimage` needs `appimagetool` on
`PATH` (or `APPIMAGETOOL=<path>`).

## CI

Everyday CI (`.github/workflows/build.yml`) runs on pull requests into `main`
and pushes to `main`: the licence, credit, workflow and asset checks on Linux,
and the full tests and packages for Windows and Linux. **macOS is not built in
CI**: it is built, tested, signed and notarized by hand on a Mac. A change to
documentation only skips the builds. The one required check is `ci-ok`.

A release:

1. On the Mac: `cargo xtask release-mac` makes the signed, notarized DMG, its
   update archive and its feed entry (`dist/release/entries/macos-*.json`).
2. Push the version tag (`vX.Y.Z`). `.github/workflows/release.yml` builds
   Windows (zip + MSI) and Linux (tar.gz, `.deb`, AppImage) and attaches them,
   with their feed entries, to the run. (A manual run can also build macOS on
   a hosted Mac, if the macOS secrets below exist; it is off by default.)
3. Put the run's `entries/*.json` next to the Mac's in `dist/release/entries/`,
   run `cargo xtask feed` on the Mac, and attach `feed.json`, the DMG and every
   package to the GitHub release.
4. Also attach the fixed-name copies (`Zornov-mac.dmg`, `Zornov-windows.msi`,
   `Zornov-linux.AppImage`, `Zornov-linux.deb`). zornov.com's download
   buttons link to `releases/latest/download/<that name>`, so they start the
   download straight away and keep working for every new version.

Secrets the release workflow reads:

| Secret | Used for |
| --- | --- |
| `STUDIO_UPDATE_KEY` | the update signing key (hex) |
| `WIN_SIGN_THUMBPRINT` | optional, self-hosted Windows runner with the certificate |
| `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD`, `NOTARY_KEY_P8`, `NOTARY_KEY_ID`, `NOTARY_ISSUER` | only for the optional hosted macOS build |

Nothing is pushed or published by the workflow.

## Crash reports

Zornov saves a report for a panic or a native crash (segfault, abort, Windows
exception) in `~/Library/Application Support/Zornov/crashes/` (Windows:
`%LOCALAPPDATA%\Zornov\crashes`, Linux: `$XDG_STATE_HOME/zornov/crashes`). It
holds the version, system, time, the message or signal and a backtrace, with
your home folder shown as `~`; no projects, media or keys. **Nothing is sent
by default.** If `crash_endpoint` in `packaging/release.conf` (or
`STUDIO_CRASH_ENDPOINT`) is an http(s) URL, the next start asks once whether
to send the waiting reports there (Send / Don't send / Not now); "Offer to
send" in Settings ▸ About turns the question off. The endpoint receives a
`POST` of the report as `text/plain`.

Native crash reports have raw addresses; keep the unstripped build (or
`dsymutil` output) of each release to read them. The handler covers the main
thread and threads started by Rust on macOS and Linux (glibc); on Linux
musl the report has the signal but no addresses.
