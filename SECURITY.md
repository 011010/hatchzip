# Security policy

## Supported versions

HatchZip has not published versioned releases yet. Security fixes are applied to the `main` branch.

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's [private vulnerability reporting](https://github.com/xevrion/hatchzip/security/advisories/new). Do not open a public issue.

You can expect an initial response within seven days. Once the issue is confirmed, a fix is prepared privately and published together with an advisory. Reporters are credited unless they prefer otherwise.

When reporting, attach a minimal crafted archive that reproduces the problem rather than a real download.

Findings from automated scanners are accepted only when they have been confirmed to be real and reachable in HatchZip.

## Threat model

HatchZip opens archives downloaded from anywhere and writes their contents to disk, and it deletes the archive as it goes. A crafted archive is the main attack vector.

| Component | Exposure |
|---|---|
| Archive parsing and decompression | The `zip` crate and its decoders process untrusted input. |
| Output paths | Entry names decide where files are written. |
| Symlinks and permissions | Entries can carry symlinks and Unix mode bits. |
| Journal and saved tail | Plain files next to the archive, readable and writable by the same user. |
| Network | None. No telemetry, accounts or update checks. |

### In scope

- Writing any file outside the output folder, through `..` paths, absolute paths, symlinks or anything else.
- Restoring setuid, setgid or sticky bits.
- An archive that makes HatchZip delete or truncate data it has not extracted and verified, including overlapping-entry tricks.
- An archive whose declared sizes let it write far more than the space check accounted for.
- Memory safety issues or code execution triggered by a crafted archive.

### Out of scope

- Attacks that require running code as the same user, including editing the journal or saved tail by hand.
- Disk exhaustion that the space check reports correctly before starting.
- Bugs in third-party decoders that do not affect HatchZip's own behavior; report those upstream.
