# Security policy

Please report security issues privately to the repository owner before opening
a public issue. Include reproduction steps, affected platform, and the
smallest useful log or trace. Do not include audio recordings or model files
that contain personal data.

TDT processes audio locally and copies recognized text to the system
clipboard. Treat clipboard contents and local run logs as sensitive.

## Updates

The optional updater contacts GitHub Releases over HTTPS (`Hi9841/tdt` by
default, or `TDT_GITHUB_REPO`). It downloads `TDT-Setup.exe` and runs it
per-user with `/SILENT /CLOSEAPPLICATIONS /NORESTART`. There is no code
signing yet. Trust the GitHub release assets and the SHA256SUMS.txt file
attached to the same release.

Disable the updater with `TDT_DISABLE_UPDATES=1`. Recording and transcription
never use the network.
