# Security policy

## Supported versions

Security fixes go into the latest release, and into `main` until the next one. Older releases are not patched; update to the latest from the [releases page](https://github.com/xevrion/hatchzip/releases/latest).

| Version | Supported |
|---|---|
| Latest release | Yes |
| Older releases | No |

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's [private vulnerability reporting](https://github.com/xevrion/hatchzip/security/advisories/new) (the Security tab, then "Report a vulnerability"). Do not open a public issue.

You can expect an initial response within seven days. Once the issue is confirmed, a fix is prepared privately and published together with an advisory. Reporters are credited unless they prefer otherwise.

When reporting, attach a minimal crafted archive that reproduces the problem rather than a real download.

Findings from automated scanners are accepted only when they have been confirmed to be real and reachable in HatchZip.

## What HatchZip does and does not do

- It works only with local files: it reads the archive it is given, writes the extracted files, and shrinks or deletes the archive as it goes.
- It makes no network connections. It has no networking code and no networking library; CI runs `cargo deny` against a ban list of HTTP, TLS and socket crates ([deny.toml](deny.toml)), so one entering the dependency tree fails the build. The desktop app's two links, to the GitHub repository in the About window and to the latest release page under Help, Check for updates, are handed to your web browser when you click them.
- It has no telemetry and no accounts, and it never checks for updates itself. It collects nothing and sends nothing.
- It never saves archive passwords.
- The Windows setup installs per user, without administrator rights, and its changes (program files, right-click menu, "Open with" entries, `PATH`) are removed by the uninstaller.

## Verifying a download

The release files are not code-signed yet, so Windows SmartScreen, macOS Gatekeeper and a few antivirus engines that score unknown programs can warn about them. To check that a file is genuine:

- **Checksum:** every release file has a `.sha256` file next to it. Compare with `Get-FileHash <file>` in PowerShell, `sha256sum -c <file>.sha256` on Linux or `shasum -a 256 -c <file>.sha256` on macOS.
- **Build provenance:** every release file from 0.1.3 on has a signed attestation that it was built by this repository's release workflow from the release tag. Check it with `gh attestation verify <file> --repo xevrion/hatchzip`, and add `--signer-workflow xevrion/hatchzip/.github/workflows/release.yml --source-ref refs/tags/v<version>` to also require that workflow and that tag.
- **No networking in the programs:** before a release is published, its Windows and macOS programs are checked to import no networking library or socket function ([no-network.py](.github/scripts/no-network.py)).
- **From source:** `cargo install hatchzip` builds the command-line tool from the published source.

[Verify your download](https://hatchzip.dev/verify.html) has the commands in full and the SHA-256 of every file in the latest release, each with a link to its VirusTotal report.

Dependencies are checked in CI against the RustSec advisory database, an allow list of licenses, and crates.io as the only source. The repository's practices are scored by [OpenSSF Scorecard](https://scorecard.dev/viewer/?uri=github.com/xevrion/hatchzip).

## Threat model

HatchZip opens archives downloaded from anywhere and writes their contents to disk, and it deletes the archive as it goes. A crafted archive is the main attack vector.

| Component | Exposure |
|---|---|
| Archive parsing and decompression | The `zip`, `sevenz-rust2` and `tar` crates, zlib-rs and UnRAR process untrusted input. |
| Output paths | Entry names decide where files are written. |
| Symlinks and permissions | Entries can carry symlinks and Unix mode bits. |
| Journal and saved tail | Plain files next to the archive, so they can arrive with a download. A resume uses them only for an extraction started on this computer, which records its output folder in the user's profile, or when the output folder is named again. It acts only on the archive's own volumes and the output folder. |
| Files HatchZip creates | Each extracted file is first written as `<name>.hatchzip-part`, and the journal and saved tail through `<name>.tmp`. These are created only where nothing exists, never through a link. |
| Network | None. No telemetry, accounts or update checks. |

### In scope

- Writing any file outside the output folder, through `..` paths, absolute paths, symlinks or anything else.
- Writing through a link, or over an existing file, at a name HatchZip creates: a file's `.hatchzip-part` name, or the temporary file of the journal or the saved tail.
- Restoring setuid, setgid or sticky bits.
- An archive that makes HatchZip delete or truncate data it has not extracted and verified, including overlapping-entry tricks.
- An archive whose declared sizes let it write far more than the space check accounted for.
- Memory safety issues or code execution triggered by a crafted archive.
- A journal or saved tail delivered next to an archive that makes a resume touch anything outside the archive's own volumes and the output folder, or delete or replace a file in the output folder that the extraction did not write.
- A release file whose checksum or attestation does not match, or a dependency that opens network connections.

### Out of scope

- Attacks that require running code as the same user, including editing the journal, the saved tail or the record of output folders in the user's profile while HatchZip runs.
- Disk exhaustion that the space check reports correctly before starting.
- Bugs in third-party decoders that do not affect HatchZip's own behavior; report those upstream.
- Antivirus detections of the unsigned release files on their own; see [Verify your download](https://hatchzip.dev/verify.html).
