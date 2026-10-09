---
name: web-frontend
description: The rules for anything a browser renders in the dusk repository - the docs site's pages, Jinja overrides, CSS, JavaScript and SVG drawings, the home page above all. Which layout techniques are required and which are banned, how a page is checked in Chromium, WebKit and Firefox at phone sizes and on a real phone before it merges, what to do when someone finds a bug on their phone, and what "done" means for a page. Use it before writing or changing anything under docs/ that is not plain Markdown prose, before fixing a bug someone found on a phone or a narrow screen, and before reviewing a branch that touches docs/docs/assets/.
---

# Web and frontend work in dusk

The docs site is MkDocs Material, published to GitHub Pages from `master` by
`.github/workflows/docs.yml`. The home page is `docs/docs/index.md` (Markdown
with `md_in_html` and `attr_list`, kept editable by people), rendered through
`docs/docs/assets/home.html`, styled by `assets/stylesheets/landing.css`, with
`assets/javascripts/landing.js` and the drawings in `assets/landing/`.

Every working agreement in `dusk-developer` holds here as it does in Rust: no
comments in CSS, JavaScript, SVG or templates; class names, custom properties
and file names are placeholders until the user passes them; every word on a
page goes to review; a fixup folds into the commit it fixes; nothing personal
goes in the repository.

## Past failure

I built the home page - about 1,100 lines of CSS and 410 of JavaScript - and
on the evening it merged I shipped five more pull requests, most of them fixing
something the user found on their phone minutes after the last one merged.
Every fix patched the symptom with the technique that caused it. The fleet
drawing was laid out by script, so each phone bug became another branch in
the script. The hero's icons were placed absolutely against a box whose height
depended on its text, so each phone bug became another media query with
offsets measured from the buttons. The stack's line used the viewport's height
as a ruler, so its dot jumped when the Android toolbar hid, and the fix moved
that one dot to `svh` and left the same ruler under the sun, the headings and
the sticky menu. I checked everything in headless Chromium, which has no
toolbar, no touch scrolling and is not WebKit, and the only build the user
could open on a phone was the live site, which deploys on merge. The page was
never responsive. It was patched at the widths someone had complained about.

## The browser lays out the page

### 1. Script never lays anything out

JavaScript does not compute or write a position, a size, a `viewBox`, a
`transform`, or an SVG geometry attribute (`x`, `y`, `cx`, `cy`, `r`, `d`,
`points`, `width`, `height`). It does not read the viewport's size
(`innerWidth`, `innerHeight`) or any element's geometry (`offset*`, `client*`,
`scrollY`, `getBBox`, `getComputedTextLength`, `ResizeObserver` rects), and it
has no `scroll` or `resize` listener and no `requestAnimationFrame` loop.

What script may do is set **state**: toggle a class or a `data-*` attribute
from an `IntersectionObserver` entry, a click or a key, and read
`matchMedia` for a user preference (`prefers-reduced-motion`,
`prefers-color-scheme`, `hover`, `pointer`) - never for a width. CSS decides
what that state looks like. A decorative pointer effect may read the rect of
the one element it turns, inside the pointer handler, and set a custom property
that CSS turns it with - nothing else.

**One question, one mechanism.** If two pieces of the page need to know which
section is current, one observer answers and both read its answer.

### 2. Flow layout, narrowest first

Layout is normal flow, grid and flex. The base rules are the narrowest layout;
wider layouts are added with `min-width` media queries or container queries.
There are no `max-width` media queries.

Rows that may be incomplete are `flex-wrap` with `justify-content`, or
`grid-template-columns: repeat(auto-fit, minmax(min(100%, <size>), 1fr))` - never
a per-item `grid-column` or `order` to centre the last row. Type and spacing
that scale use `clamp()` with a `rem` floor and ceiling, so zoom still works.

### 3. Absolute positioning only inside a box of known shape

`position: absolute` or `fixed` is allowed only when the containing block's
size does not depend on its content - it has an `aspect-ratio`, or the
positioned element covers it with `inset: 0`. A decoration that has to sit near
some text goes in the same grid as that text, in its own area or track, so it
moves when the text wraps. `position: sticky` is the tool for "stays in place
while the page scrolls".

### 4. No length that encodes another element's size

A length is a token (a custom property on `.dusk-landing` or `:root`), a
`clamp()`, `min()` or `max()` of tokens and `rem`, `em`, `cqi` or `svh`, or a
`calc()` of those. A literal that restates something else's size - a button's
width inside an icon's `left`, half a dot's diameter as its offset, the height
of a hill as a `bottom` - is a token, named and derived. The same literal never
appears in two rules.

### 5. Breakpoints are few, shared, and set by content

A page-level change of shape uses Material's breakpoints and no others: `30em`
(480px), `45em` (720px), `60em` (960px), `76.25em` (1220px), `100em`, `125em`.
A component that changes shape does it with a container query on its own
wrapper (`container-type: inline-size`), at the width where its content stops
fitting, stated in the commit message. A breakpoint is never added at the width
where something was reported broken - that is a patch, see
[A bug found on a phone](#a-bug-found-on-a-phone). Breakpoints never appear in
JavaScript.

### 6. The viewport's height is not a ruler

On a phone the viewport's height changes while the reader scrolls, as the
browser's toolbar hides and shows. So there is no bare `vh` in the CSS: `svh`
for anything placed or sized against the screen while it is read (a sticky
`top`, a minimum height that must fit), `lvh` for a background that must cover,
`dvh` only for something meant to resize as the toolbar moves and never for
something the reader is looking at while scrolling. All three have been in
every major engine since 2022 (Safari 15.4, Firefox 101, Chrome 108), so no
`vh` fallback line is written. Script never reads the viewport's
height (rule 1).

### 7. Scroll effects are the browser's

Something that holds its place is `position: sticky`. Which section is current
is an `IntersectionObserver` toggling a class. Something that changes in
proportion to the scroll is a CSS scroll-driven animation
(`animation-timeline: view()` or `scroll()`) inside
`@supports (animation-timeline: view())`; where it is not supported the page
shows the effect's finished, static state, and nothing in script stands in for
it. Effects change only `transform`, `opacity`, colour and `filter`, never
layout, so nothing moves under the reader's finger. Under
`prefers-reduced-motion: reduce` every effect shows its static state.

### 8. The Markdown stays editable

Adding, removing or reordering an item in `index.md` - a platform, a card, a
layer, an icon - needs no CSS change. So CSS selects by the classes and
`data-*` attributes the Markdown writes (`{ .name data-layer="..." }`), never by
position: no `:nth-child`, `:nth-of-type`, `:first-child`, `:last-child` or
`:only-child`, and no `> p` aimed at a wrapper Markdown generated. An item's
identity - its colour, its place - comes from an attribute on it.

### 9. Drawings carry their own layout

An SVG drawing has a fixed `viewBox` written in the file and scales with
`width: 100%; height: auto`. Script never rewrites it. If a narrow container
needs a different composition, that is a second drawing, authored for it, and a
container query shows one or the other. No text in a drawing renders smaller
than 12 CSS px at any checked size - text in an SVG shrinks with the drawing, so
a drawing composed for 1200 units wide cannot carry labels at 320px, and that is
what the second drawing is for. A drawing is not made to fit by widening it
past its container and cropping it.

### 10. The page is whole without JavaScript

With script disabled every section renders at its final size, and with script
enabled nothing below it moves when the script runs. Script adds state and
motion; it never finishes the layout.

### 11. A symptom is not hidden

No painting the page's background colour over something to make it look
dimmer, no `overflow: hidden`, `clip` or `clip-path` on a section to hide what
spills out of it, no `display: none` at the one width something collides. Each
of these assumes what is behind or beside the thing, and each breaks the day
that changes.

## The stack is MkDocs Material, plain CSS and plain JavaScript

Pages are Markdown, Jinja overrides of Material's templates, plain CSS and
plain JavaScript loaded as files, with no build step of their own. I do not add
a framework (React, Vue, Svelte, Tailwind or any other), a bundler, a
`package.json`, or a script or stylesheet from a CDN to the site. If something
cannot be done within that, I stop and say so; a new toolchain is the user's
decision.

## A bug found on a phone

1. Reproduce it in an engine at the size where it was seen. If no engine can
   reproduce it - the toolbar, touch momentum, iOS Safari - say so.
2. Find the rule above that the code which produced it breaks. Almost always
   one does.
3. If one does, I do not patch it in the same technique: no breakpoint at the
   width it broke, no new case in a layout script, no new offset, no `vh`
   swapped for `svh` in one place while the same ruler stays in others. The fix
   is that component rebuilt within the rules, which is a redesign, so I tell
   the user the bug, the rule, every other place the same technique is used, and
   what rebuilding the component involves - and wait.
4. If none does, I fix it, and I add the condition that would have caught it to
   the checks below, so the sweep catches its siblings too.
5. While the branch is unmerged, the fix folds into the commit it fixes.

Code already in the tree that breaks these rules is not rewritten in passing; I
list it to the user once. A change never adds a violation, and never extends a
component that already has one.

## Checking a page

The working agreements keep tests out unless the user asks for them. Opening a
page in browser engines and measuring it is not a test suite: it is to a page
what `cargo check` is to a crate, and it is required before a page change is
handed over. None of it adds test code to the repository. Whether the checks
become a committed script run by pre-commit or CI is the user's decision; until
then the script lives in a scratch directory outside the repository, and its
output goes in the pull request.

Builds and browsers run at idle priority (`nice -n 19`), and the site is built
outside the repository: `cd docs && nice -n 19 uv run mkdocs build --strict -d <scratch>/site`.

### Static checks

Run from the repository root. Each command prints nothing, or every hit is one
the rules allow and the pull request names it.

```sh
js=docs/docs/assets/javascripts
css=docs/docs/assets/stylesheets
grep -rnE '"(scroll|resize)"|requestAnimationFrame|setInterval' $js
grep -rnE 'inner(Width|Height)|outer(Width|Height)|scroll(X|Y|Top|Left)\b|offset(Top|Left|Width|Height)|client(Top|Left|Width|Height)|getBBox|getComputedTextLength|contentRect' $js
grep -rnE 'setAttribute\(\s*"(x|y|x1|x2|y1|y2|cx|cy|r|rx|ry|d|points|width|height|viewBox|transform)"|style\.(top|left|right|bottom|width|height|inset|margin|transform)\b' $js
grep -rnE 'matchMedia\([^)]*(width|height)' $js
grep -rnE '(^|[^a-z])[0-9.]+vh\b' $css
grep -rnE '@media[^{]*max-(width|height)' $css
grep -rnoE '@media[^{]*min-width:\s*[0-9.]+[a-z]+' $css | grep -vE 'min-width:\s*(30|45|60|76\.25|100|125)em'
grep -rnE ':(nth-child|nth-last-child|nth-of-type|nth-last-of-type|first-child|last-child|only-child)' $css
grep -rnE 'getBoundingClientRect' $js
grep -rnE 'position:\s*(absolute|fixed)|overflow(-x|-y)?:\s*(hidden|clip)|clip-path' $css
```

The last two always need reading by eye: a `getBoundingClientRect` is allowed
only inside a pointer handler for the element it turns (rule 1), and each
absolute, fixed or clipped element needs the box of known shape rule 3 asks for
and no hidden symptom (rule 11).

### Engines

Chromium, WebKit and Firefox, through Playwright, at a pinned version named in
the pull request. WebKit is the nearest thing to iOS Safari a Linux machine
has, and Chromium alone is not a check of the web. If WebKit or Firefox is
missing, I say so and ask before installing it - installing browsers or their
system libraries is a host action - and I never let Chromium stand in for
them.

Each size is emulated as the device: mobile viewport, touch, its device pixel
ratio. Playwright cannot emulate a mobile viewport in Firefox, so there it is
the size and the pixel ratio only, and the pull request says so.

| viewport (CSS px) | what it stands for | pixel ratio |
|---|---|---|
| 280 × 653 | a folded phone's cover screen | 3 |
| 320 × 568 | the WCAG reflow width; iPhone SE (first generation) | 2 |
| 360 × 800 | a common Android phone | 3 |
| 375 × 667 | iPhone SE (second and third generation) | 2 |
| 390 × 844 | iPhone 12 to 14 | 3 |
| 412 × 915 | Pixel | 2.625 |
| 430 × 932 | a large iPhone | 3 |
| 844 × 390 | a phone held sideways | 3 |
| 768 × 1024 | a tablet, upright | 2 |
| 1024 × 768 | a tablet on its side, a small laptop | 2 |
| 1280 × 800 | a laptop | 1 |
| 1440 × 900 | a desktop | 1 |
| 1920 × 1080 | a desktop | 1 |

Beyond those, a sweep: every width from 280 to 1440 CSS px in Chromium, and
every 4px across the same range in WebKit and Firefox.

### What every size must show

At every device size and every sweep width, in the light and the dark scheme:

1. No horizontal scroll: `scrollWidth` of the document is not more than its
   `clientWidth`.
2. Nothing a reader reads or presses overlaps anything else - each line box of
   text, each button and link, each label and node in a drawing - checked with
   animations paused, and again with each moving decoration's box grown by the
   farthest its animation takes it.
3. No text renders below 12 CSS px, text inside a drawing included.
4. Every button, and every link that is not inside a sentence, is at least
   24 × 24 CSS px (WCAG 2.2, success criterion 2.5.8).
5. Every section is the same height, within 1px, with script disabled as with
   it enabled.
6. Under `prefers-reduced-motion: reduce`, nothing animates and every
   scroll-linked thing shows its static state.

The checking script may measure anything; the rules on geometry are for the
page.

I look at a screenshot of every device size myself, in both schemes. A
measurement does not see something that is ugly without overlapping.

### What no engine shows: a real phone, before merge

No headless engine has a toolbar that hides as the page scrolls - `svh`, `lvh`
and `dvh` all equal `vh` in emulation, so emulation cannot tell them apart - nor
touch scrolling with momentum, nor a page scrolled on another thread ahead of
its script, and none of them is iOS Safari. Anything sticky, fixed,
scroll-linked or sized to the viewport is therefore seen on a real phone before
it merges.

The live site is not a preview: the docs workflow publishes `master` on every
push, so a phone check after merge is a check on what every visitor already
has. Before the pull request merges, the user opens the branch's build on their
phone - from `mkdocs serve --dev-addr 0.0.0.0:8000` on the local network, or
from `localhost:8000` on a phone on USB after `adb reverse tcp:8000 tcp:8000`,
where `chrome://inspect` in a desktop Chrome gives DevTools for the phone's own
Chrome. Serving to the network and forwarding
ports are host actions: I ask each time. If no iPhone was used, the pull request
says iOS Safari was not checked on a device.

## Done

A page change is done when:

1. Prettier, djLint and `mkdocs build --strict` pass - the existing hooks.
2. Every static check prints nothing, or each hit is one the rules allow and
   the pull request names it.
3. The checks pass in Chromium, WebKit and Firefox at every device size and
   across the sweep, in both schemes, with motion and with reduced motion.
4. I have looked at every device size myself.
5. The user has seen it on their phone, from the branch, before it merges.
6. The pull request says what was checked - engines and their versions, sizes,
   schemes - and what was not. Said once, there, and not again in every reply.
