# Opens a Word document and reports its structure, one KEY=VALUE per line.
#
# Microsoft Word is the oracle for a .docx: a package our own reader
# accepts can still be one Word refuses to open. The report is written to
# a file in UTF-8, because the console's code page cannot hold the style
# names Word answers with.
param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Report
)

$ErrorActionPreference = "Stop"
$lines = @()

$word = New-Object -ComObject Word.Application
$word.Visible = $false
$word.DisplayAlerts = 0
try {
    $doc = $word.Documents.Open($Path, $false, $true, $false)
    $lines += "OPENED=ok"
    $lines += "PARAGRAPHS=$($doc.Paragraphs.Count)"
    $lines += "TABLES=$($doc.Tables.Count)"
    $lines += "LISTS=$($doc.Lists.Count)"
    $lines += "SHAPES=$($doc.InlineShapes.Count)"
    $lines += "FIELDS=$($doc.Fields.Count)"
    $lines += "HYPERLINKS=$($doc.Hyperlinks.Count)"

    if ($doc.Fields.Count -gt 0) {
        $lines += "FIELD1=$($doc.Fields.Item(1).Code.Text.Trim())"
    }
    if ($doc.InlineShapes.Count -gt 0) {
        $shape = $doc.InlineShapes.Item(1)
        $lines += "SHAPE1W=$([math]::Round($shape.Width, 1))"
        $lines += "SHAPE1H=$([math]::Round($shape.Height, 1))"
    }
    if ($doc.Tables.Count -gt 0) {
        $table = $doc.Tables.Item(1)
        $lines += "TABLE1ROWS=$($table.Rows.Count)"
        $lines += "TABLE1COLS=$($table.Columns.Count)"
        $lines += "TABLE1A1=$($table.Cell(1, 1).Range.Text.Trim())"
    }

    $styles = @()
    foreach ($paragraph in $doc.Paragraphs) {
        $styles += $paragraph.Style.NameLocal
    }
    $lines += "STYLES=$(($styles | Select-Object -Unique) -join '|')"

    $text = $doc.Content.Text
    $lines += "TEXTLEN=$($text.Length)"
    $lines += "TEXT=$($text.Substring(0, [Math]::Min(400, $text.Length)))"

    $doc.Close($false)
} catch {
    $lines += "ERROR=$($_.Exception.Message)"
} finally {
    $word.Quit()
}

[System.IO.File]::WriteAllLines($Report, $lines, (New-Object System.Text.UTF8Encoding $false))
