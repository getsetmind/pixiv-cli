[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

function Invoke-CargoCheck {
    param([string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed (exit $LASTEXITCODE)."
    }
}

Push-Location -LiteralPath $repoRoot
try {
    $metadataOutput = & cargo metadata --no-deps --format-version 1 --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed (exit $LASTEXITCODE)."
    }
    $metadata = ($metadataOutput -join "`n") | ConvertFrom-Json
    $testAttributes = '#\s*\[\s*(?:[A-Za-z_]\w*::)*test\b|\bcfg\s*\(\s*test\b|\bcfg!\s*\(\s*test\b'
    foreach ($package in $metadata.packages) {
        if ($metadata.workspace_members -notcontains $package.id) {
            continue
        }
        $sourceRoot = Join-Path (Split-Path -Parent $package.manifest_path) 'src'
        if (Test-Path -LiteralPath $sourceRoot) {
            $testCode = Get-ChildItem -LiteralPath $sourceRoot -Recurse -File -Filter '*.rs' |
                Select-String -Pattern $testAttributes
            if ($testCode) {
                $locations = $testCode | ForEach-Object { "$($_.Path):$($_.LineNumber)" }
                throw "Test attributes in production sources: $($locations -join ', '). Move tests to tests/."
            }
        }
    }
    Invoke-CargoCheck -CargoArguments @('fmt', '--all', '--', '--check')
    Invoke-CargoCheck -CargoArguments @('clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings')
    Invoke-CargoCheck -CargoArguments @('test', '--workspace', '--locked')
    Invoke-CargoCheck -CargoArguments @('build', '--workspace', '--release', '--locked')
}
finally {
    Pop-Location
}
