# Verifies a .docx with tools that are not MarkPop's own: the .NET XML
# parser and the .NET ZIP reader. Two witnesses that are not written by
# us catch what our own reader and writer agree on wrongly.
param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Report
)

$ErrorActionPreference = "Stop"
$lines = @()

# Windows PowerShell does not load the ZIP reader by default.
Add-Type -AssemblyName System.IO.Compression.FileSystem

try {
    $zip = [System.IO.Compression.ZipFile]::OpenRead($Path)
    try {
        $names = $zip.Entries | ForEach-Object { $_.FullName }
        $lines += "ENTRIES=$($names -join ',')"

        foreach ($entry in $zip.Entries) {
            if ($entry.FullName -like "*.xml" -or $entry.FullName -like "*.rels") {
                $reader = New-Object System.IO.StreamReader($entry.Open())
                $text = $reader.ReadToEnd()
                $reader.Close()
                # Throws when the XML is not well-formed.
                $null = [xml]$text
                $lines += "XML_OK=$($entry.FullName)"
            }
        }
    } finally {
        $zip.Dispose()
    }
    $lines += "VERIFIED=ok"
} catch {
    $lines += "ERROR=$($_.Exception.Message)"
}

[System.IO.File]::WriteAllLines($Report, $lines, (New-Object System.Text.UTF8Encoding $false))
