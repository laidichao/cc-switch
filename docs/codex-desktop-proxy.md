# Codex desktop domain proxy (Windows)

This optional feature uses CC Switch's saved HTTP/HTTPS upstream proxy for
Codex sign-in and official HTTPS requests that respect the Windows system proxy.
It is disabled by default. It does not install a service, download a core, or
launch Codex. Administrator permission is required only to add/remove the fixed
Windows hosts mapping; the main CC Switch interface retains its normal privilege.

## Use

1. Save an HTTP/HTTPS upstream in **Settings > Advanced > Global upstream proxy**.
2. Enable **Use for Codex desktop sign-in and official requests** in the same section.
3. Approve the hosts update permission prompt, then restart Codex normally. Its backend must support the
   `features.respect_system_proxy` setting (the development version inspected
   during implementation was `0.162.0-alpha.17.2`).
4. Verify sign-in, account details, settings, and an official subscription turn.
   The ready label confirms only the local listener, system PAC, and backend
   configuration key and fixed cloud hosts mapping.

CC Switch publishes a PAC at a private loopback listener. HTTPS requests to
`openai.com`, `chatgpt.com`, `oaistatic.com`, `oaiusercontent.com`, and their
subdomains use this listener; other destinations use `DIRECT`. The listener only
accepts CONNECT to these hosts on port 443. Existing local model routes are
unchanged. The configured upstream is contacted directly, with its existing TLS
certificate validation and credentials. Target TLS is passed through unchanged.
No target request bodies or account credentials are logged.

## Scope and coexistence

- This is domain routing, not process routing: another application visiting the
  same HTTPS domains also uses this proxy. Ordinary video/download destinations
  are not sent to the upstream server.
- Some desktop WebSockets use independent Node transports that do not inherit
  the system PAC. For the observed public cloud WebSocket only,
  `codex-cloud-backend.chatgpt.com` is mapped to `127.0.0.2` and `::1` in hosts.
  CC Switch binds those loopback addresses on port 443 and forwards raw bytes to
  the same hostname through the existing upstream CONNECT implementation.
  Target TLS, SNI, authentication, and WebSocket frames stay end to end; no CA is
  installed and no target TLS is decrypted. Future independent transports or new
  domains are not automatically covered.
- Existing active manual system proxies or PACs cause activation to fail. VPN
  software that changes these settings later takes precedence; CC Switch reports
  the mismatch instead of overwriting the VPN settings. VPN routes are not edited.
- While enabled, Windows automatic proxy discovery is temporarily disabled and
  nonmatching domains are direct. The previous flags/PAC are backed up.
- SOCKS remains available for ordinary CC Switch upstream requests. This first
  desktop feature requires HTTP/HTTPS CONNECT. Disable it before clearing the
  upstream or switching to SOCKS.
- Both fixed listeners must bind before configuration changes. Port conflicts,
  existing cloud hosts entries, malformed owned markers, and denied permission
  abort activation. Other hosts entries and their original bytes remain intact.
- The fixed mapping affects all programs resolving this exact hostname, not only
  Codex. Other destinations continue using the existing PAC/domain rules.
- Existing applications may cache proxy decisions. The inspected Codex backend
  caches successful decisions for 60 seconds. Restarting Codex refreshes its
  configuration; switching this setting cannot rewrite an existing connection.

## Restore and failures

Before changing settings, CC Switch writes a recovery record containing the
original system proxy options, original Codex feature key and hosts ownership UUID. It does
not back up tokens, provider credentials, or the whole Codex configuration.

Disabling or normally quitting requests administrator permission to remove the
owned hosts block, then restores settings only if their current values
still belong to this feature, then stops the listener and its tunnels. Provider
selection, authentication, and unrelated Codex settings remain under their
existing owners. Changes by another proxy application are not overwritten.

If restoration fails during activation, the listener and recovery record remain
available and the settings page displays the error so disabling can retry.
An abnormal process termination cannot perform cleanup: restoration occurs at
the next CC Switch startup, with a permission prompt if the owned hosts block
remains. Until then the PAC and fixed hosts mapping can point to stopped listeners.
If permission is denied on disable, the listener and recovery record remain so
the user can retry. Normal quit and update/restart cleanup are cancelled on
restoration failure. Disable the feature before uninstalling or downgrading;
removing CC Switch while its hosts mapping remains can leave that domain unavailable.
If the application is forcibly terminated or system exit
does not allow restoration, the next startup performs recovery.
The enable preference is retained across normal quits.

## Validation boundary

Compilation and static checks do not validate real desktop sign-in or prove that
every Codex connection uses the proxy. End-to-end desktop acceptance must be
reported separately from local readiness and upstream reachability.
