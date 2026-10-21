# We Replaced Our Screenshot Pipeline

*How a single Rust binary cut our docs build from 4 minutes to 400 milliseconds.*

---

Every documentation page we publish used to go through the same three steps: render Markdown
to HTML, spin up a headless browser, wait for it to settle, and screenshot the result. It
worked, sort of. It also cost us a 300 MB browser dependency in the build image, added
roughly ninety seconds of wall clock per run, and handed us a PNG that nobody could
restyle, re-search, or paste into a slide with working text.

## What we wanted instead

The requirements were not exotic:

- text that stays **text**, so it can be selected, searched and read by a screen reader;
- output that looks identical on a CI runner and on a laptop;
- no browser, no system APIs, no C libraries;
- a build that finishes before you finish reading the commit message.

> The moment the output is a bitmap, every downstream consumer inherits the limitations of
> a bitmap. Fonts get baked in. Text stops being text.

## The pipeline now

There are three stages, and each one is a pure function with its own test suite:

```text
Markdown text
  -> parse     (src/parse)   -> Document
  -> layout    (src/layout)  -> Scene
  -> engine    (src/engine)  -> SVG | PNG | WebP
```

The interesting part is the middle stage. `Scene` is a flat list of primitives — text runs,
rectangles, lines — with coordinates in base units. Everything downstream is a serializer
over that list, which is why adding WebP output took an afternoon and not a quarter.

### Measuring without a font file

The obvious dependency for a layout engine is the system's font metrics, but that makes
output depend on whatever happens to be installed. Instead we ship a deterministic advance
table, so the same document breaks its lines the same way everywhere.

| Property | Screenshot pipeline | MarkPop |
|---|---|---|
| Cold start | ~90 s | ~0.4 s |
| Dependencies | browser + system libs | none |
| Text | flattened to pixels | real text |
| Determinism | depends on installed fonts | byte-for-byte |
| Remote images | fetched | never fetched |

## Results

- Build time for the full docs site dropped from four minutes to under two seconds.
- The published SVG is 38 KB and searchable in the browser.
- Our CI image lost 300 MB and about forty transitive packages.
- Nobody has asked for the PNGs back.

### Checklist

- [x] Replace the HTML intermediate with a direct Markdown parser
- [x] Ship a deterministic metrics table instead of reading system fonts
- [x] Add a bitmap engine only where compatibility demands it
- [ ] Add PDF export without leaving the Rust world
