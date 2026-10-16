# Security Policy

ShotCraft reads the screen, the clipboard, image files and `.shotcraft` projects, and can upload captures to an endpoint you configure.
Treat image files, project files, clipboard contents, command-line arguments and upload responses as untrusted input.

## Supported versions

Fixes land on `main`; once releases exist, only the latest release receives security fixes.
ShotCraft is pre-1.0, so a fix ships in a new release rather than as a backport.

## Reporting a vulnerability

Report privately through GitHub's private vulnerability reporting: <https://github.com/mmartinez/shotcraft/security/advisories/new>.
Do not open a public issue, discussion or pull request containing a working exploit, a secret or a malicious file.
Please allow time for a fix before publishing details.

## What to include

- The ShotCraft version or commit, and how you installed it.
- Operating system, desktop environment or window system (X11, Wayland, Windows, macOS) and display scaling.
- The command or action that triggers the problem, with exact reproduction steps.
- Expected and actual behaviour, plus any crash output or logs.
- The security impact and what access an attacker needs.

Remove credentials, upload tokens, personal paths and private screenshots from logs before sending them.
If a malicious file is needed to reproduce the problem, first describe its format, size and SHA-256 hash, and agree on a private way to transfer it.

## Scope

- Decoding images and `.shotcraft` projects (JSON with an embedded PNG), including oversized dimensions, decompression bombs and hangs.
- Clipboard reads and writes, and drag and drop into other applications.
- Uploads: the request built from your settings, and parsing of the response.
- Downloading the OCR models.
- File paths for saving, history and project files, including overwrites and symlinks.
- Launching modes as child processes and parsing command-line arguments.
- Dependencies, CI, packaging and release artifacts.

General bugs without a security impact belong in the normal issue tracker.

## Current limitations

- Upload settings, including any authorization headers, are stored in plain text in the settings file in your user configuration folder.
  On Linux and macOS ShotCraft makes its configuration and data folders readable only by your user account.
- Capture history (screenshots, projects, and copies staged for drag and drop) is stored unencrypted in your user data folder.
  An entry stays until you delete it or it expires; expired entries are removed the next time you capture or open the history.
- Redaction and cropping are edits: a `.shotcraft` project, including the one kept with each history entry, still contains the original, unredacted capture.
  Share exported images, not project files.
