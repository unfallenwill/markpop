# The Cost of a Screenshot

*Notes on rendering, fidelity, and the quiet virtues of files you can still edit.*

A screenshot is a claim that the document ended there. It flattens a structure — headings,
lists, quotes — into a rectangle of ink, and asks the reader to trust that nothing was cut
off, nothing was reflowed, and nothing was lost in the transfer.

## Flattening

Flattening is not inherently bad. For a thumbnail, or a preview in a feed, it is exactly
right. The trouble starts when the flattened artifact becomes the only artifact.

Once text is pixels, three things follow. It can no longer be searched. It can no longer
be reflowed, so a narrow screen either scrolls forever or crops. And it can no longer be
restyled, so a change of brand means rendering every page again.

## The alternative

The alternative is not a faster screenshot. It is a document that was never a picture to
begin with.

```rust
let svg = markpop::render("# Hello\n\nWorld");
```

That call returns a string of markup. The text in it is text. You can select it, search
it, scale it to any size, or hand it to a different renderer entirely. The geometry lives
in a flat list of primitives, which means the serializer is the only part that has an
opinion about what the output should look like.

> The document is the source of truth. The image is one rendering of it.

## Two columns

Two columns change how a document reads. Narrow measures force shorter sentences and
faster paragraphs. A page of dense prose becomes a page you can scan.

Balanced columns share one continuous flow: the engine finds the shortest height that
fits the requested number of columns, and divides the text evenly between them. Nothing
is lost and nothing is reordered — the reader simply reaches the bottom of the first
column slightly earlier than the second.

## What is lost

Honesty requires listing the losses. There is no syntax highlighting, because a
highlighter is a separate program with its own opinions about tokenization. There is no
remote image fetching, because a renderer that reaches out to the network is a renderer
that can hang. And there is no HTML execution, for the same reason a document should not
be able to run code on the machine that displays it.

Everything else — emphasis, code, tables, quotes, task lists, footnotes of your own
making — is there, and it is yours to restyle.

## Closing

The question to ask of any rendering tool is not how fast it is, but what it leaves
behind. A good tool leaves the document intact.
