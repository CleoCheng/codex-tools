$ErrorActionPreference = 'Stop'
$scriptPath = Join-Path $PSScriptRoot '../src-tauri/src/auto_retry_capacity_windows.ps1'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Resolve-Path -LiteralPath $scriptPath).Path, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -ne 0) { throw ($parseErrors | Out-String) }

function Assert-True($condition, $message) {
    if (-not $condition) { throw $message }
}

foreach ($functionName in @('Test-RetryName', 'Test-CapacityText', 'Test-BoundsContain')) {
    $definition = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $functionName
    }, $true)
    . ([scriptblock]::Create($definition.Extent.Text))
}

Assert-True (Test-RetryName 'Retry') 'English Retry must qualify.'
Assert-True (Test-RetryName '重试') 'Chinese Retry must qualify.'
Assert-True (-not (Test-RetryName 'Send')) 'Send must not be treated as capacity Retry.'
Assert-True (Test-CapacityText 'Selected model is at capacity. Please try a different model.') 'Capacity banner must qualify.'
Assert-True (Test-CapacityText 'server_overloaded') 'Structured capacity text must qualify.'
Assert-True (-not (Test-CapacityText 'You have hit your usage limit.')) 'Quota exhaustion must not enter capacity Retry.'
Assert-True (-not (Test-CapacityText 'Network error')) 'Network error must not enter capacity Retry.'

function New-Bounds($left, $top, $right, $bottom) {
    [pscustomobject]@{Left=$left; Top=$top; Right=$right; Bottom=$bottom; IsEmpty=$false}
}
$outer = New-Bounds 0 0 800 600
$inner = New-Bounds 100 100 700 500
$outside = New-Bounds 100 100 900 500
Assert-True (Test-BoundsContain $outer $inner) 'Retry card must stay inside its target scope.'
Assert-True (-not (Test-BoundsContain $outer $outside)) 'A button outside the target window must be rejected.'
Write-Output 'PASS: capacity banner, Retry label, parser and bounded scope fixtures.'
