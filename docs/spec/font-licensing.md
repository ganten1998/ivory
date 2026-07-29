# Font Licensing Research: Courier New vs. Courier Prime for a Commercial Cross-Platform App

Context: commercial macOS/Linux/Windows desktop app (Ivory), sold pay-what-you-can including $0.
The app currently styles text as `"Courier Prime", "Courier New", Courier, monospace` and the repo
already vendors `CourierPrime-{Regular,Bold,Italic,BoldItalic}.ttf` plus the full upstream
`CourierPrime-master` repo (including `OFL.txt`). The Linux `.deb` staging tree installs the four
TTFs to `/usr/share/fonts/truetype/courier-prime/`.

Research date: 2026-07-28.

---

## 1. Courier New — ownership, bundling, system-font reference, cost

**Ownership.** The original Courier design (Howard "Bud" Kettler, mid-1950s, for IBM) was never
trademarked or design-protected by IBM — the *name "Courier" and the design* are public domain.
**Courier New** is Monotype's digitization (from IBM Selectric golf-ball masters), produced for
Microsoft and shipped as a system font since Windows 3.1. The *font software* (the .ttf files) is
proprietary, owned by Monotype; "Courier New" is a Monotype trademark. MyFonts (Monotype's retail
arm) lists Monotype as designer/publisher.
Sources: [Wikipedia – Courier (typeface)](https://en.wikipedia.org/wiki/Courier_(typeface)),
[Microsoft Typography – Courier New](https://learn.microsoft.com/en-us/typography/font-list/courier-new),
[MyFonts – Courier New (Monotype)](https://www.myfonts.com/collections/courier-new-font-monotype-imaging/).

**Can you bundle/redistribute the .ttf with a commercial app? NO — not without a paid license.**
The standard EULA embedded in the shipped fonts limits use to the licensee's own workstation and
prohibits copying or distribution ("You may not copy or distribute this software"). The only
freely-redistributable channel that ever existed — Microsoft's "TrueType core fonts for the Web"
(1996–2002, included `courier32.exe`) — was terminated in August 2002, and even that EULA only
permitted redistribution **in the original, unmodified .exe/.sit.hqx packaging**, with the explicit
condition that the fonts may not be used to "add value to commercial products." Extracting the TTFs
and shipping them inside a commercial app is squarely prohibited.
Sources: [Wikipedia – Core fonts for the Web](https://en.wikipedia.org/wiki/Core_fonts_for_the_Web),
[Wikipedia – TrueType core fonts for the Web](https://en.wikipedia.org/wiki/TrueType_core_fonts_for_the_Web).

**Can you reference it by name as a system font if installed? YES — on every OS.** Naming a font in
a font stack (`QFont("Courier New")`, CSS `font-family`) copies no font software; the end user's own
OS license covers rendering with fonts installed on their machine. No license from Monotype is
needed to *reference* an installed font, and nominative use of the name in a fallback list or font
picker is fine.
- **Windows**: ships Courier New in every version since 3.1 ([Microsoft font list](https://learn.microsoft.com/en-us/typography/font-list/courier-new)).
- **macOS**: still ships it — Apple's macOS Tahoe font list includes "Courier New Version 5.00.2x"
  plus Bold, Italic, Bold Italic (5.00.x), alongside PostScript Courier/Courier Oblique 19.0d1e1
  ([Fonts included with macOS Tahoe](https://support.apple.com/en-us/122869); same for
  [Sequoia](https://support.apple.com/en-us/120414) and [Sonoma](https://support.apple.com/en-us/108939)).
- **Linux**: NOT shipped. Users may opt in via `ttf-mscorefonts-installer` (Debian/Ubuntu contrib),
  which downloads the original 2002 .exe packages and extracts them locally — legal for the user,
  but an app vendor cannot ship the result.

**What would a redistribution license cost/require?** Retail on MyFonts: Courier New styles start at
**$50.99/style, $135.99 for the 4-style family**, but that is the *desktop* license (design use).
Embedding in an application requires a separate **App license** (per-application-title embedding
license; priced separately at checkout, historically a multiple of the base price per title), and
OS-style redistribution/OEM requires a negotiated Monotype agreement — Monotype now pushes annual
subscription contracts for enterprise embedding (Monotype Fonts; the old Library Subscription was
$119.99/yr for *desktop-only* use, no redistribution). In short: bundling Courier New legally means
recurring negotiation/cost with Monotype and is disproportionate for a pay-what-you-can app.
Sources: [MyFonts – Courier New](https://www.myfonts.com/collections/courier-new-font-monotype-imaging/),
[Monotype Foundry Support – App License](https://foundrysupport.monotype.com/hc/en-us/articles/10840068991636-App-License),
[Vendr – Monotype pricing](https://www.vendr.com/marketplace/monotype-imaging),
[Business Wire – Monotype Library Subscription](https://www.businesswire.com/news/home/20160125005126/en).

**Verdict:** never ship Courier New files; referencing it by name as a fallback is free and legal on
all three OSes (it will simply not resolve on most Linux systems).

---

## 2. Courier Prime — license confirmed, bundling confirmed, RFN status, fidelity

**License: SIL Open Font License 1.1 — CONFIRMED** from three independent places:
- Upstream repo [quoteunquoteapps/CourierPrime](https://github.com/quoteunquoteapps/CourierPrime)
  (`OFL.txt`: "This Font Software is licensed under the SIL Open Font License, Version 1.1").
- The official page: "released under the [OFL] license"
  ([quoteunquoteapps.com/courierprime](https://quoteunquoteapps.com/courierprime/)).
- The vendored copy in this repo:
  `/Users/ganten/Library/CloudStorage/Dropbox/Archive/Ivory/fonts/CourierPrime-master/OFL.txt`.

Designed by **Alan Dague-Greene** for **John August**, published by **Quote-Unquote Apps**. Family =
Regular, Bold, Italic, Bold Italic (exactly the four TTFs already in the repo).

**Commercial bundling: allowed.** OFL permits use, modification, and redistribution, including
bundling with sold software ("may be bundled, redistributed and/or sold with any software" — OFL 1.1
condition 1; see §5 below for the FAQ confirmation).

**Reserved Font Name status:** the **current** upstream `OFL.txt` copyright line is
"Copyright 2015 The Courier Prime Project Authors (https://github.com/quoteunquoteapps/CourierPrime)."
— it declares **no Reserved Font Name** (verified by grep of the local vendored file: the only
"Reserved Font Name" occurrence is the license's generic DEFINITIONS text). Early 2013 releases did
declare "Courier Prime" as an RFN, so treat the name as reserved out of caution. Practical
implications either way:
- Shipping the **unmodified** TTFs: no constraint — the OFL FAQ confirms unmodified Original
  Versions keep their name with no restriction.
- If you ever **modify/subset** the fonts: rename the modified fonts (do not call them
  "Courier Prime") and release the modified font files under OFL.

**Visual/metric closeness to Courier New:** Courier Prime was designed as "a better Courier":
per the official page it "**matches the metrics of Courier and Courier Final Draft, so you can often
swap it out one-for-one**" — same 600-unit advance width and 12-point layout metrics as
Courier New, so line lengths and page counts match. Visually it is a faithful typewriter-serif
Courier (unlike Cousine/Liberation Mono), slightly darker/crisper than Courier New's thin strokes,
with a true italic instead of Courier New's sloped design. It is the standard Courier replacement in
screenwriting tools for exactly this reason.
Sources: [quoteunquoteapps.com/courierprime](https://quoteunquoteapps.com/courierprime/),
[GitHub – quoteunquoteapps/CourierPrime](https://github.com/quoteunquoteapps/CourierPrime),
[Wikipedia – Courier (typeface)](https://en.wikipedia.org/wiki/Courier_(typeface)).

---

## 3. Other open metric/visual-compatible alternatives

| Font | License | Metric-compatible w/ Courier New | Visual fidelity to Courier New |
|---|---|---|---|
| **Courier Prime** | OFL 1.1 | Yes (12pt one-for-one swap) | **High** — true Courier look, better italic, slightly darker |
| **Cousine** (Croscore, Steve Matteson/Ascender, ships with ChromeOS) | Apache 2.0 | Yes (designed metrically compatible) | **Low-moderate** — not a Courier clone; shares its design with Liberation Mono 2.x (grotesque/sans-leaning monospace) |
| **Liberation Mono** (Red Hat) | 1.x: GPLv2 + font-embedding exception; **2.x+: OFL 1.1** (rebased on Croscore/Cousine) | Yes | **Low-moderate** — Wikipedia: "styled closer to Liberation Sans than to Courier New" |
| **Nimbus Mono PS** (URW++, Ghostscript base-35) | Current release: **AGPLv3** with a document-embedding exception only (older Nimbus Mono L: GPL/AFPL) | Yes (PostScript Courier metrics) | **High** — URW's faithful Courier clone for PostScript substitution |
| **Courier 10 Pitch** (Bitstream) | v2.0 donated to the X Consortium in 1990 under a permissive MIT-style license (note: the retail "Courier 10 Pitch BT" on MyFonts is a separate commercial license) | Approximately (10-pitch Courier metrics) | **High** — heavier than Courier New; closer to the original IBM Courier |
| **TeX Gyre Cursor** (GUST) | GUST Font License (LPPL-like, free) | Yes (based on Nimbus Mono) | High (Nimbus-derived) |

Bundling notes:
- **Apache 2.0 (Cousine)** and **OFL (Courier Prime, Liberation Mono 2+)**: unproblematic for
  closed-source commercial bundling; include license text/attribution.
- **AGPLv3 (Nimbus Mono PS)**: the appended exception covers embedding the font in
  PostScript/PDF *documents*, not bundling in applications; shipping it inside a proprietary app
  means distributing AGPL components — legally workable (the font stays AGPL, app unaffected in
  most readings) but a compliance headache and some readings are hostile. Avoid; URW sells
  commercial licenses if ever needed.
- **Courier 10 Pitch**: only the X11-donation Type 1/derived versions are free; do not confuse with
  the MyFonts retail cut.

Sources: [Wikipedia – Croscore fonts](https://en.wikipedia.org/wiki/Croscore_fonts),
[Google Fonts – Cousine](https://fonts.google.com/specimen/Cousine),
[Wikipedia – Liberation fonts](https://en.wikipedia.org/wiki/Liberation_fonts),
[ArchWiki – Metric-compatible fonts](https://wiki.archlinux.org/title/Metric-compatible_fonts),
[urw-base35-fonts LICENSE (AGPLv3 + embedding exception)](https://github.com/ArtifexSoftware/urw-base35-fonts),
[Wikipedia – Courier (typeface)](https://en.wikipedia.org/wiki/Courier_(typeface)),
[FSF Directory – Croscorefonts](https://directory.fsf.org/wiki/Croscorefonts).

---

## 4. Recommendation for Ivory

The safe, zero-cost, zero-negotiation strategy — and the app is already 90% there:

1. **Bundle Courier Prime (all four TTFs) as the default font on all three OSes.** It is the only
   open font that is both metric-compatible AND visually faithful to Courier New, and it is OFL —
   explicitly bundlable with sold software at any price point (pay-what-you-can, including $0 and
   paid, is irrelevant to OFL). The repo already contains the correct files
   (`fonts/CourierPrime-*.ttf`) and the `.deb` already stages them to
   `/usr/share/fonts/truetype/courier-prime/`.
2. **Load the bundled TTFs at runtime** (`QFontDatabase.addApplicationFont(...)` in Qt/PySide6)
   rather than relying on system installation, so macOS/Windows builds render identically without an
   installer step. Keep the `.deb` system-font install for Linux (fine under OFL) or switch it to
   runtime loading too — either is compliant.
3. **Keep the existing fallback stack** `"Courier Prime" → "Courier New" → Courier → monospace`.
   Referencing Courier New/Courier by name is legally free everywhere; it resolves on macOS and
   Windows and harmlessly falls through to `monospace` on Linux. Never ship Courier New files, and
   verify no packaging script ever copies user-system fonts into the bundle.
4. **Compliance to add (the only real gap):** ship `OFL.txt` alongside the TTFs in every
   distributed artifact (macOS .app bundle, Windows installer, and the `.deb` — e.g.
   `/usr/share/doc/<pkg>/` or next to the fonts), and mention "Courier Prime © The Courier Prime
   Project Authors, SIL Open Font License 1.1" in the About box. That satisfies OFL condition 2
   (copyright + license text must accompany copies). The OFL FAQ explicitly blesses an About
   box/credits mention plus the bundled license file.
5. **Offer a custom-font preference** (free text or system font picker) — no licensing implication:
   selecting a user-installed font is covered by the user's own font licenses.
6. **Do not subset/modify** the bundled TTFs; if optimization ever requires it, rename the modified
   fonts (avoid "Courier Prime" out of RFN caution) and ship the modified files under OFL.
7. Repo hygiene (optional): three duplicate copies of the family exist (`fonts/`,
   `fonts/courier-prime-files/`, `fonts/CourierPrime-master/fonts/ttf/`); pick one canonical set +
   `OFL.txt` for the build.

This is 100% safe for commercial sale on macOS, Windows, and Linux with zero payments to anyone.

---

## 5. OFL constraints when the app itself is sold — verified

From the [OFL FAQ](https://openfontlicense.org/ofl-faq/) (SIL, also at scripts.sil.org/OFL):

- **Fonts may NOT be sold standalone**: "Neither the Font Software nor any of its individual
  components, in Original or Modified Versions, may be sold by itself" (OFL 1.1, condition 1).
- **Bundling with commercial/sold software IS allowed**: FAQ — "Yes, you can do this with both the
  Original Version and a Modified Version of the fonts" (examples given include text editors and
  commercial apps). The price of the app (including $0 / pay-what-you-can) does not matter.
- **The app does NOT have to be open source**: "Only the portions based on the Font Software are
  required to be released under the OFL." The OFL does not virally extend to the application.
- **Obligations**: each copy must be accompanied by the copyright statement + license notice +
  license text (bundled `OFL.txt` + About-box credit satisfies this); Reserved Font Names only bind
  *modified* versions — unmodified originals keep their name freely.
- **Documents/output** created with the fonts are unrestricted.

---

## Source list

- https://en.wikipedia.org/wiki/Courier_(typeface)
- https://en.wikipedia.org/wiki/Core_fonts_for_the_Web
- https://en.wikipedia.org/wiki/TrueType_core_fonts_for_the_Web
- https://learn.microsoft.com/en-us/typography/font-list/courier-new
- https://support.apple.com/en-us/122869 (macOS Tahoe font list; also 120414 Sequoia, 108939 Sonoma)
- https://www.myfonts.com/collections/courier-new-font-monotype-imaging/
- https://foundrysupport.monotype.com/hc/en-us/articles/10840068991636-App-License
- https://www.vendr.com/marketplace/monotype-imaging
- https://www.businesswire.com/news/home/20160125005126/en
- https://quoteunquoteapps.com/courierprime/
- https://github.com/quoteunquoteapps/CourierPrime (and raw OFL.txt)
- https://openfontlicense.org/ofl-faq/
- https://en.wikipedia.org/wiki/Croscore_fonts
- https://fonts.google.com/specimen/Cousine
- https://directory.fsf.org/wiki/Croscorefonts
- https://en.wikipedia.org/wiki/Liberation_fonts
- https://wiki.archlinux.org/title/Metric-compatible_fonts
- https://github.com/ArtifexSoftware/urw-base35-fonts (raw LICENSE: AGPLv3 + embedding exception)
- Local verification: /Users/ganten/Library/CloudStorage/Dropbox/Archive/Ivory/fonts/CourierPrime-master/OFL.txt
