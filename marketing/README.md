# Dragent marketing and documentation

A Next.js App Router site for Dragent’s desktop workspace, supported agent workflows, downloads, pricing, and documentation.

## Run locally

```sh
npm ci
npm run dev -- --hostname 127.0.0.1 --port 3100
```

Open <http://127.0.0.1:3100>. For a production check:

```sh
npm run lint
npx tsc --noEmit
npm run build
npm run start -- --hostname 127.0.0.1 --port 3101
# In another terminal:
node scripts/check-site.mjs http://127.0.0.1:3101
```

## Design direction

The homepage puts the product first: a centred headline, a large native interface preview, a selectable walkthrough, and focused feature captures. Copy explains what a developer does, where the work runs, and how to get started. It avoids customer endorsements, invented speed metrics, and unsupported provider parity claims.

The visual direction was revisited after the first proposal: restrained dark surfaces, clear product positioning, generous product demonstrations, and workflows explained through the interface.

Additional research informed content and documentation structure:

- [Cursor](https://cursor.com/) and [Cursor documentation](https://cursor.com/docs): focused navigation and task-oriented entry points.
- [Linear](https://linear.app/) and [Linear documentation](https://linear.app/docs): typography, spacing, and progressive information hierarchy.
- [Warp](https://www.warp.dev/): terminal and agent workflow positioning.

References reviewed on October 27, 2026. These links are research references, not affiliations or endorsements.

## Product media

`public/screens/` contains optimized WebP captures from Dragent’s native preview application. The application uses production feature renderers with seeded sample data. Some workspace preview scenes include a lightweight demonstration shell. Captions identify the sample project.

`public/demos/` contains four silent, continuous recordings of the native interface being operated with a sample project. They show:

- Coordination: switching between activity and plan, then drafting direction to the lead.
- Workspace: composing and refining a task prompt, including verification instructions.
- Reviews: opening Files, selecting a diff line, drafting a comment, and opening Checks and Activity.
- Remote: switching between host resources and the installed-agent view.

These are captured from the running native window framebuffer. They contain no still-image crossfades, camera zooms, desktop captures, or unrelated notifications. The controls and editor use the application’s production renderers and action handlers. UI input is automated for repeatability; keyboard replay uses the production editor handler. The sample project does not execute a provider agent, establish a real SSH connection, or publish a review or message.

`public/demos/manifest.json` records each clip’s duration, captured-frame count, and chapter timings. The website reads that metadata for its playback label and active chapter. Videos load only after a visitor requests playback; native controls, full-size viewing, and English text tracks are available.

Generate still captures from the repository root:

```sh
cd dragent
DRAGENT_DEMO_CAPTURE=1 cargo run -p dragent --example ui_preview -- /private/tmp/dragent-site-captures
cd ../marketing
node scripts/prepare-product-screens.mjs /private/tmp/dragent-site-captures
```

To record a native sample window, select its scene and enable continuous recording. For example:

```sh
cd dragent
DRAGENT_DEMO_CAPTURE=1 DRAGENT_PREVIEW_INTERACTIVE=pr_overview \
  DRAGENT_PREVIEW_RECORD=1 DRAGENT_PREVIEW_RECORD_SECONDS=90 \
  cargo run -p dragent --example ui_preview -- /private/tmp/dragent-recordings/reviews
```

Operate the window’s controls while it is recording. The output is timestamped `frame-INDEX-MILLISECONDS.png` files, captured independently of desktop visibility. Available scenes include `teams_dark`, `pr_overview`, `demo_ssh_system`, and `demo_project_solo`. The optional `input.jsonl` file accepts local keyboard replay commands such as `{"select_all":true}` and `{"text":"Draft text"}`. This is confined to the isolated preview application and drives its normal editor action handler; it does not invoke agents or publish text.

After recording the four named folders, encode the native frames on macOS:

```sh
cd marketing
swift scripts/create-product-demos.swift /private/tmp/dragent-recordings public/demos
```

An optional `actions.json` in each recording folder supplies chapter labels and times. Without it the encoder retains the recording’s full duration. Each action uses `label`, `before`, and `after` in capture milliseconds. The video generator requires macOS AVFoundation. Full-view still images are resized to 1800 px wide WebP; detail images use the crop manifest in `scripts/prepare-product-screens.mjs`. Agent glyphs come from the app’s existing agent assets and identify the respective products.

## Documentation

Documentation is statically rendered at `/docs/[slug]`, with a server-rendered article body and small client components for navigation, search, copy actions, and the table of contents. `/docs#chapter` links redirect to the matching article for compatibility.

- `src/components/docs/guides.tsx`: current workflow guides.
- `src/components/docs/content.tsx`: retained reference material.
- `src/components/docs/catalog.tsx`: navigation, search metadata, route resolution, and article ordering.
- `src/components/docs/primitives.tsx`: reading, table, code, and callout styles.

Keep feature claims aligned with the implementation. In particular, external agent billing is separate from Dragent billing; remote hosts have platform and authentication requirements; account isolation and model adapters differ by provider; a successful automation launch does not mean its task completed.

## Release configuration

`src/lib/site.ts` holds download URLs, account destination, and contact details. `NEXT_PUBLIC_APP_URL` overrides the account portal; `NEXT_PUBLIC_DOCS_URL` overrides the documentation destination. The current default account portal remains `https://beta.dragent.dev`.

The four existing macOS/Windows stable installer URLs returned HTTP 200 during verification. Linux desktop has no download URL and is explicitly labelled unavailable. Pricing amounts and the annual discount were checked against server plan migrations; the website does not create or change billing products. Legal policy text remains separate from product documentation.

Before a public deployment, confirm the intended account portal environment, release channel, support address, billing terms, and legal policy details. Deploying is a separate action from building and locally verifying the site.

## Verification for this revision

The October 27, 2026 revision passed ESLint, TypeScript, and the production build. Browser checks covered the four videos, expanded preview, product navigation, mobile navigation, docs search and article routing, mobile docs navigation, installer links, monthly/annual pricing, and theme switching. The production server passed 24 browser assertions. The static route/link checker verified 45 pages and 3,011 internal links without failures, including heading IDs, image links, and unknown-guide 404 responses.

### Real Codex session

`public/demos/codex-native.mp4` is an actual screen recording of a single Codex session in the running Dragent app on October 28, 2026. It shows typing a read-only request, code inspection, working status, and streamed findings, with the workspace sidebar visible throughout. An opaque branded editorial panel is baked into the video and poster to conceal the file explorer. No UI preview or scripted agent output is used for this clip. The original capture was trimmed and cropped to the exact app window; playback is real time, with no zooms or crossfades.

The sidebar in the clip is a still of the current sidebar laid over the recorded one, so the clip shows today's design: render it with `DRAGENT_PREVIEW_ONLY=coordination_sidebar_light,marketing_sidebar cargo run -p dragent --example ui_preview -- <dir>`, crop `marketing_sidebar.png` to its first 504 px, then run `swift scripts/overlay-sidebar.swift public/demos/codex-native.mp4 sidebar.png out.mp4 0 34 212 757` and replace the clip and its poster (`public/screens/codex.webp`, the first frame). The first preview scene in a run is a throwaway.

`prepare-agent-recording.swift` performs the crop/export using AVFoundation without launching the app. Raw desktop footage stays outside the public folder. The inline and enlarged players advance to the next workflow when playback ends, wrapping from the last workflow to the first. The other walkthrough clips retain their sample-data labels.
