# `build/macos/`

## `tangent.entitlements`

Hardened-runtime entitlements, passed to **both** `codesign` invocations in
`scripts/build-macos.sh` — the inner executable and the outer bundle — on the
Developer ID path **and** the ad-hoc path.

### Why the file exists

Until 2026-08-16 there was no entitlements file in this repo at all
(`grep -rn entitlement` returned nothing) and `codesign` was never given one,
while the build has always passed `--options runtime`. That combination was
correct for an app that touches no protected resource, and Tangent touched none.

The Recorder view opens a camera, a microphone, and third-party plugin binaries.
Under the hardened runtime, each of those is refused **without a prompt** unless
the matching key is present, and no row ever appears in
System Settings → Privacy & Security. The device list is simply empty and
nothing anywhere says why. That is the single most expensive failure mode in the
whole feature, because it looks like a bug in the capture code.

### Why all four keys landed at once

Camera and microphone are needed by the capture work; the two hosting keys are
needed months later. They ship together anyway because **changing entitlements
changes the CDHash**, so each change costs a re-sign and a re-notarization.
Doing it once for the whole feature saves a full release cycle and gives up
nothing.

It does **not** invalidate anyone's existing permissions. Verified by
experiment: signing this bundle with and without these keys produces
byte-identical designated-requirement blobs (`codesign -d -r`, same sha256);
only the CDHash differs. The requirement names the bundle id, the Apple anchor,
the Developer ID marker OIDs and the team OU — no entitlement term and no
specific certificate leaf — so Developer ID TCC grants survive both entitlement
changes and certificate renewal.

### What is deliberately absent

`com.apple.security.app-sandbox`. Tangent is a Developer ID app distributed
outside the App Store, it writes takes to a directory the user picks, and it
loads third-party plugin binaries. Sandboxing would require security-scoped
bookmarks for the output directory and would break plugin loading outright, for
no benefit this distribution channel asks for.

### The format trap

AMFI parses this file with its own strict XML reader, not CFPreferences:

* the `<!DOCTYPE …>` line is **required**
* comments must live **inside** `<dict>`

A comment between `<?xml?>` and `<plist>` produces

```
Failed to parse entitlements: AMFIUnserializeXML: syntax error near line 7
```

which names a line number but not the rule. Found by bisection.

### Verifying a build

```sh
codesign -d --entitlements - dist/Tangent.app      # all four keys
codesign -d --entitlements - dist/Tangent.app/Contents/MacOS/tangent
codesign -d -v dist/Tangent.app 2>&1 | grep -i flags   # expect runtime
```

`scripts/build-macos.sh` asserts all of this itself and fails the build if the
signature comes back without the keys, because a silently-unentitled release is
indistinguishable from a working one until a user tries to record.
