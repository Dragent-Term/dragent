# Licensing


## What license applies

This repository's own code is licensed under the
**GNU Affero General Public License, version 3 only** (SPDX identifier:
`AGPL-3.0-only`).

- Full text: [`LICENSE`](./LICENSE)
  (canonical version: <https://www.gnu.org/licenses/agpl-3.0.html>)
- Copyright holder: Nimbletech Ltd (Copyright (c) 2026)

## What the AGPL-3.0-only allows

The AGPL-3.0 lets anyone dealing in the software do anything with it,
**including commercial use**. You may, without restriction:

- **Use** it — run it privately, inside a business, or as a paid
  hosted/SaaS offering;
- **Copy** it;
- **Modify** it — make changes and new works based on it;
- **Merge and publish** it, including embedding it in your own products;
- **Distribute** copies, modified or not; and
- **Charge** for copies, support, warranty, or hosting around the software.

There is no commercial-use restriction: commercial use, paid hosting,
embedding in paid products, and internal business/production use are all
permitted — provided you meet the copyleft conditions below.

## The network rule (Section 13)

This is what makes the AGPL different from permissive licenses and from the
ordinary GPL: if you run a modified version of this software **over a
network and let other users interact with it there**, you must **offer all
those users the Corresponding Source** of your version — under the AGPL,
under Sections 4–6 conveyance rules applied to network use. Running a
modified copy as a hosted service without distributing binaries does not
escape the obligation; the source offer must be real (downloadable,
machine-readable, no paywall or registration games). See `LICENSE`
Section 13 for the binding terms.

## Conditions

When you convey the software (modified or not), or run a modified version
for others over a network, you must:

- give recipients (or network users, per Section 13) a copy of the license
  and a way to get the Corresponding Source;
- preserve copyright, license, and NOTICE attributions;
- cause modified files to carry prominent notices stating that you changed
  them, with a relevant date; and
- license the entire conveyed work, as a whole, under the AGPL-3.0-only,
  retaining attribution notices as described in `LICENSE` Sections 4–6.

The software comes "AS IS", without warranty of any kind.

## Patent grant and retaliation

Each contributor grants you a worldwide, royalty-free patent license to the
contributor's essential patent claims to make, use, sell, offer for sale,
import, and otherwise run, modify, and propagate their contributions
(see `LICENSE` Section 11). If you convey a covered work while knowingly
relying on a patent license, you must extend it or make the Corresponding
Source available (Section 11, paragraphs 3–5). Your rights under the license
terminate if you initiate patent litigation alleging that the software
infringes, and a discriminatory patent license arrangement tied to the
covered work is prohibited (Sections 8–10). See `LICENSE` Sections 8–11 for
the binding terms.

## Trademark limitation

The license gives no permission to use the trade names, trademarks, service
marks, or product names of the licensor, except as required for reasonable
and customary use in describing the origin of the Work and reproducing the
content of the NOTICE file.

## Third-party components keep their own licenses

This repository bundles third-party components (libraries, fonts, icons,
skill packs, shaders) that remain under **their own** licenses — MIT,
Apache-2.0, ISC, SIL Open Font License 1.1, NCSA/LLVM, BSD-style, and
similar. The AGPL-3.0-only above applies to this project's own code; it
does **not** relicense third-party components. In particular, the `warpui`
and `warpui_core` crates stay under their MIT license (see
`dragent/crates/LICENSE-WARPUI`).

Attribution and full third-party license texts live in the existing notice
files, which are preserved unchanged apart from their preambles:

- [`dragent/NOTICE`](./dragent/NOTICE)
- [`dragent/THIRD_PARTY_NOTICES.md`](./dragent/THIRD_PARTY_NOTICES.md)
- per-component `LICENSE*` / `ATTRIBUTION*` files alongside the components
  (e.g. `dragent/crates/`, `dragent/app/assets/`, `dragent/skills/`)

Keep those files intact and comply with their terms (e.g. retain copyright
notices, ship the required license texts, do not relicense OFL fonts) when
you redistribute this software.

## Contributor terms

By contributing to this repository you agree that your contributions are
licensed inbound under the same terms — the GNU Affero General Public License,
version 3 only (**inbound = outbound**). This keeps the whole tree under one
license. Every contributor must sign [`CLA.md`](./CLA.md) (sign-once via the
bot; see [`CONTRIBUTING.md`](./CONTRIBUTING.md)), which also grants
Nimbletech Ltd a broad, irrevocable license to the contribution, including
the right to sublicense it. Contributors keep copyright in their own work;
do not add per-file author tags and do not strip existing third-party
headers.

## Open Source

This project is **Open Source**: the AGPL-3.0 is approved by the Open Source
Initiative and the Free Software Foundation, and it imposes no restriction
on fields of endeavor, including commercial use — copyleft conditions apply
equally to every field. You may describe this project as "open source"
without qualification.

## License history

This repository's own code was PolyForm-NC, briefly MIT, then Apache-2.0, now AGPL-3.0-only for network-copyleft monetization leverage.

---

*This file is an explanatory summary, not legal advice. The binding terms are
in [`LICENSE`](./LICENSE). Have counsel review before relying on these terms
in a commercial decision.*
