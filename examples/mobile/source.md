# Quick Recipe: Ship a README Image

**Target:** a shareable long image for social and chat

A README screenshot is usually 800 px wide, which is unreadable on a phone. This recipe
renders the same document at phone width with larger body text.

```bash
markpop README.md --canvas mobile -o share.svg
```

## What you get

- a 750 px wide continuous image, no pagination
- 24 px body text instead of 16 px
- 32 px padding instead of 48 px

## Tuning the output

| Goal | Flag | Value |
|---|---|---|
| Wider canvas | `--width` | `800` |
| Bigger text | `--font-size` | `26` |
| Looser leading | `--line-height` | `1.7` |
| Dark background | `--theme` | `dark` |
| Higher resolution | `--zoom` | `150` |

## Combine with a raster format

If the destination does not accept SVG, produce a lossless WebP instead:

```bash
markpop README.md --canvas mobile --zoom 150 -f webp -o share.webp
```

## Checklist

- [x] Render at phone width
- [x] Bump the body text to 24 px
- [x] Export lossless WebP for chat apps
- [ ] Add the project logo to the header
