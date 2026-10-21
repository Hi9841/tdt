# Security policy

Please report security issues privately to the repository owner before opening
a public issue. Include reproduction steps, affected platform, and the
smallest useful log or trace. Do not include audio recordings or model files
that contain personal data.

TDT processes audio locally and copies recognized text to the system
clipboard. Treat clipboard contents and local run logs as sensitive.

## Updates

The optional updater reads the latest release of `Hi9841/tdt` from the
GitHub API over HTTPS. It downloads `TDT-Setup.exe`, checks its size and the
SHA-256 digest that GitHub publishes for the asset, and runs it for the
current user with `/S`. Setup checks the SHA-256 of the TDT.exe that it
carries before it installs it. There is no code signing yet, so trust rests
on the GitHub release.

Disable the updater with `TDT_DISABLE_UPDATES=1`. Recording and transcription
never use the network.
