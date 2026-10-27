#!/usr/bin/env bash
# Regenerate every rendered file under examples/ from its source.md.
#
#   examples/render.sh          # render in place
#   examples/render.sh --check  # verify the committed SVG output is up to date
#
# Override the binary with MARKPOP=/path/to/markpop.
#
# Every document is rendered in all three output formats. The SVG output is
# deterministic, so --check compares it byte for byte. The bitmap output depends
# on the fonts installed on the machine that renders it, so those files are
# checked for existence only.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

check=0
[ "${1:-}" = "--check" ] && check=1

bin="${MARKPOP:-}"
if [ -z "$bin" ]; then
  for candidate in target/release/markpop target/release/markpop.exe \
                   target/debug/markpop target/debug/markpop.exe; do
    if [ -x "$candidate" ]; then bin="$candidate"; break; fi
  done
fi
if [ -z "$bin" ] && command -v markpop >/dev/null 2>&1; then
  bin="markpop"
fi
if [ -z "$bin" ]; then
  echo "error: markpop binary not found." >&2
  echo "       run 'cargo build --release' first, or set MARKPOP=/path/to/markpop" >&2
  exit 1
fi

# Render one document in every format. `$2` is the output stem; a paginated
# canvas appends the page number to it, e.g. page.svg -> page-1.svg.
render() {
  local dir="$1" stem="$2"
  shift 2
  rm -f "examples/$dir/$stem".* "examples/$dir/$stem"-*.* 2>/dev/null || true
  local fmt
  for fmt in svg png webp; do
    "$bin" "examples/$dir/source.md" -f "$fmt" -o "examples/$dir/$stem.$fmt" "$@"
  done
  echo "  examples/$dir/$stem.{svg,png,webp}"
}

echo "rendering with $bin"

# Feature showcase: the same document through all three engines.
render showcase showcase

# The showcase as an editable Word document. A .docx is not paginated the
# way an image is, so only the showcase carries one.
"$bin" examples/showcase/source.md -f docx -o examples/showcase/showcase.docx
echo "  examples/showcase/showcase.docx"

# Use cases.
render blog-post  blog-post
render dark-docs  dark-docs  --theme dark
render print      print      --theme print
render magazine  magazine  --columns 2 --column-gap 28
render mobile    mobile    --canvas mobile

# A4 handbook: auto-paginated, one file per page.
render handbook page --canvas a4

# Chinese use cases.
render zh-blog-post zh-blog-post
render zh-dark-docs zh-dark-docs --theme dark
render zh-print     zh-print     --theme print
render zh-magazine  zh-magazine  --columns 2 --column-gap 28
render zh-mobile    zh-mobile    --canvas mobile
render zh-announcement zh-announcement --canvas mobile
# 位图以 2x 渲染：长图主要在手机高 DPI 屏上查看；SVG 保持 1x 矢量。
"$bin" examples/zh-announcement/source.md --canvas mobile --zoom 200 -f png  -o examples/zh-announcement/zh-announcement.png
"$bin" examples/zh-announcement/source.md --canvas mobile --zoom 200 -f webp -o examples/zh-announcement/zh-announcement.webp

render zh-handbook page --canvas a4

if [ "$check" = "1" ]; then
  status=0
  svg_paths=':(glob)examples/**/*.svg'

  if ! git diff --quiet -- "$svg_paths"; then
    echo "error: rendered SVG differs from the committed files." >&2
    git diff --stat -- "$svg_paths" >&2
    echo "       run examples/render.sh and commit the result" >&2
    status=1
  fi

  untracked="$(git ls-files --others --exclude-standard -- "$svg_paths")"
  if [ -n "$untracked" ]; then
    echo "error: untracked SVG under examples/:" >&2
    printf '%s\n' "$untracked" >&2
    status=1
  fi

  # Every rendered document must exist in all three formats.
  for svg in $(git ls-files -- "$svg_paths"); do
    for ext in png webp; do
      if [ ! -s "${svg%.svg}.$ext" ]; then
        echo "error: ${svg%.svg}.$ext is missing" >&2
        status=1
      fi
    done
  done

  # The showcase also ships as a Word document.
  if [ ! -s examples/showcase/showcase.docx ]; then
    echo "error: examples/showcase/showcase.docx is missing" >&2
    status=1
  fi

  [ "$status" = "0" ] && echo "check ok: examples/ is up to date"
  exit "$status"
fi
