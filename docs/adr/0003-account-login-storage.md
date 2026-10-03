# ADR 0003 — Account storage, session gating and sign-in process (M2, Windows)

Status: implemented and tested on this host; owner-run end-to-end evidence pending (see
`crates/m2-probe`). Builds on ADR 0002.

## Decisions

1. **Gate on the account, not the resolver.** The resolver only ever receives a session snapshot
   from the account service while the session is Valid and an identity is Selected. With no
   snapshot it does not start yt-dlp at all. Reason (M0): yt-dlp resolves with no or fake cookies, so
   a successful resolve says nothing about the account.
2. **Jar encrypted with an AEAD, key in Windows Credential Manager.** XChaCha20-Poly1305,
   ciphertext in a user-only directory. Credential Manager cannot hold a full jar (2.5 KB blobs);
   it holds the key. Alternative considered: DPAPI directly (simpler, no key handling). Kept the
   spec's design; DPAPI remains an acceptable swap behind the `Vault` trait.
3. **No plaintext fallback.** Vault unavailable means memory-only for this run, stated in the view,
   never a file.
4. **Sign-in in a separate helper process; cookies on its anonymous stdout pipe.** No named pipe, no
   file, no argv/env, no second event loop in the GPUI app. The helper is contained in a Job Object.
5. **Validation outside locks, results checked against an epoch.** Logout, import, identity change
   and cookie rotation each bump the epoch; anything that finishes under an older epoch is dropped.
6. **A network failure is not a logout.** Only the service saying "not logged in" (or 401/403)
   expires a session; unknown shapes are a compatibility problem.

## Consequences

- The probe and the future app must run logout in this order: playback stop, resolver cancel, lease
  clear, then `AccountService::logout()` (hooks exist for the first three).
- Revalidation is the app's job (`needs_revalidation()` / `revalidate()`): at startup, after a
  resolver `SessionExpired`, and about every 30 minutes.
- GPUI integration of the sign-in is just "spawn helper, wait, import": nothing UI-specific.
- Open: multi-channel (brand account) handling is verified against synthetic JSON only.
- Open: header scoping for media requests (see docs/m2-media-notes.md, finding 3).
