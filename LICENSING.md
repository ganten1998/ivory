# Licensing

Tangent ships two binaries under two licences. This file exists so that split
is written down once, precisely, rather than reconstructed from memory later.

| Artifact | Licence | Why |
|---|---|---|
| `Tangent.app` / `tangent` / `tangent.exe` (standalone) | **MIT** | Links nothing copyleft. |
| `ivory-core`, `ivory` and every other crate in this repo | **MIT** | Source, not binaries. |
| `Tangent.vst3` (the plugin) | **GPL-3.0-or-later** | Links NIH-plug's VST3 bindings. |

## Why the plugin is GPLv3 and the app is not

The copyleft comes from the **Rust bindings crate**, not from Steinberg.

`plugin/` reaches `vst3-sys` (RustAudio) transitively through the pinned
`nih_plug` revision, and that crate declares `license = "GPL-3.0"` in its own
`Cargo.toml`. NIH-plug's documentation is explicit about the consequence: "any
VST3 plugins built with NIH-plug need to be able to comply with the terms of the
GPLv3 license." NIH-plug itself is ISC; it is those bindings specifically that
carry the copyleft, and `Tangent.vst3` links them, so `Tangent.vst3` is GPLv3.
That is still true and nothing below changes.

**What is no longer true, corrected 2026-08-15.** This section used to say
"Steinberg dual-licenses the VST3 SDK: GPLv3, or a separate proprietary
agreement with Steinberg." That was accurate when written and stopped being
accurate on **2025-10-29**, when Steinberg relicensed the VST3 SDK to **MIT**
with VST 3.8. Verified at source: `steinbergmedia/vst3sdk/LICENSE.txt` and
`steinbergmedia/vst3_pluginterfaces/LICENSE.txt` are both plain MIT
("Copyright (c) 2026, Steinberg Media Technologies GmbH"), the GitHub API
reports `spdx_id: MIT` on both repositories, and the SDK README states that
"Licensing under GPLv3 and the Steinberg proprietary license is no longer
available." There is no host/plug-in distinction under MIT; the old SDK licence
had one.

Two consequences worth writing down, because both are load-bearing for future
work:

1. `vst3-sys`'s GPLv3 is now a **historical artifact of a 2023 crate that has
   not been touched since 2023-06-19**, not a requirement anyone still imposes.
   The MIT-licensed `vst3` crate (coupler-rs, `MIT OR Apache-2.0`) exists and is
   maintained. If `plugin/` ever moves off `nih_plug`/`vst3-sys`, the GPLv3
   quarantine could be dissolved entirely. It is blocked today only because the
   maintained successor (`nice-plug`, ISC, which uses the MIT `vst3` crate)
   requires egui 0.35 and this project is pinned at 0.33.
2. **An MIT application may host VST3 plugins.** That is what makes the Recorder
   view's plugin-hosting workstream legally possible; see `docs/RECORDER-PLAN.md`
   §8. The only surviving obligation is trademark, and it is now optional under
   MIT — see the trademark section below, which still applies.

Copyleft attaches to the **binary that does the linking**, not to source that
happens to be compiled into it. MIT is GPL-compatible, so the same MIT sources
in this repo are lawfully compiled into both the MIT standalone and the GPLv3
plugin. Nothing here needs relicensing, and contributions stay MIT.

## Why shipping both in one installer does not infect the standalone

GPLv3 section 5 defines an **aggregate**:

> A compilation of a covered work with other separate and independent works,
> which are not by their nature extensions of the covered work, and which are
> not combined with it such as to form a larger program, in or on a volume of a
> storage or distribution medium, is called an "aggregate" [...] Inclusion of a
> covered work in an aggregate does not cause this License to apply to the other
> parts of the aggregate.

The standalone and the plugin are separate executables. Neither loads, links to,
or requires the other; they communicate through nothing; each runs alone. Putting
both on one disk image is distribution on a shared medium, which is exactly what
section 5 describes.

**Three rules keep that true. Do not break them casually:**

1. **The plugin stays optional.** The installer offers it as a checkbox and the
   standalone works fully without it. A fused product that cannot be taken apart
   is a much weaker aggregation argument.
2. **They never link.** No shared dynamic library between the two, no plugin
   loading the app or vice versa, no IPC that makes one require the other. If
   they ever need to share code at runtime rather than at compile time, this
   analysis has to be redone before shipping.
3. **Each carries its own licence.** `LICENSE` (MIT) next to the app,
   `LICENSE-GPL-3.0` inside the `.vst3` bundle.

## Obligations that actually have to be met for the plugin

- Ship or offer the **complete corresponding source** for the plugin, including
  the exact NIH-plug revision it was built against. The repo is public, which
  covers this, provided releases record the commit rather than "latest".
- Include the GPLv3 text with the plugin (`LICENSE-GPL-3.0`).
- Add no further restrictions on the plugin binary.

`scripts/build-plugin.sh` is responsible for putting the licence text and the
pinned NIH-plug revision inside the bundle. If that step is ever dropped, the
release is non-compliant, quietly.

## Trademarks, which are separate from all of the above

Steinberg's VST trademark terms are not a copyright matter and apply regardless
of which SDK licence is used. The relevant one: **"VST" must not appear in the
product name.** "Tangent" satisfies this. Do not ship anything called
"Tangent VST".

## Not legal advice

This is a written-down reading of the licences, not an opinion from a lawyer.
The aggregation analysis above is the standard one and the structure follows the
conservative path, but the first release that takes money is a reasonable moment
to have someone qualified confirm it.
