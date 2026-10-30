$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$script:invokeAttempted = $false
trap {
    if (-not $script:invokeAttempted) {
        [Console]::Out.WriteLine('NOT_RETRIED')
        [Console]::Out.Flush()
    }
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class CodexToolsCapacityRetryNative {
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
}
'@

$title = $env:CODEX_TOOLS_CAPACITY_RETRY_TITLE
if ([string]::IsNullOrWhiteSpace($title)) { throw 'Missing the capacity-error conversation title.' }

$buttonCondition = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Button)
$documentType = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Document)
$titleCondition = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::NameProperty, $title)
$anyCondition = [System.Windows.Automation.Condition]::TrueCondition
$script:lastReason = 'Waiting for the capacity error and Retry button.'

function Test-CodexDesktopProcess($processPath, $expectedPath) {
    if ([string]::IsNullOrWhiteSpace($processPath)) { return $false }
    $normalized = $processPath.Replace('/', '\')
    if (-not [string]::IsNullOrWhiteSpace($expectedPath) -and
        [string]::Equals($normalized, $expectedPath.Replace('/', '\'), [StringComparison]::OrdinalIgnoreCase)) {
        return $true
    }
    return $normalized -match '(?i)\\WindowsApps\\OpenAI\.Codex_[^\\]+\\app\\(ChatGPT|Codex|Codex Desktop)\.exe$'
}

function Get-CodexDesktopProcesses {
    foreach ($process in @(Get-Process -Name 'Codex', 'ChatGPT', 'Codex Desktop' -ErrorAction SilentlyContinue)) {
        try {
            if (Test-CodexDesktopProcess $process.Path $env:CODEX_TOOLS_CAPACITY_RETRY_APP_PATH) { $process }
        } catch { }
    }
}

function Get-CapacityParent($element) {
    return [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetParent($element)
}

function Test-RetryName($name) {
    return $name -match '^(Retry|Try again|Retry response|Retry message|重试|重试回答|重新尝试)(\s*[\(\uff08].*)?$'
}

function Test-CapacityText($name) {
    if ([string]::IsNullOrWhiteSpace($name)) { return $false }
    return $name -match '(?i)(selected model.*capacity|model.*at capacity|at capacity|server[_ -]?overloaded|算力不足|容量不足)'
}

function Test-BoundsContain($outer, $inner) {
    if ($outer.IsEmpty -or $inner.IsEmpty) { return $false }
    return $inner.Left -ge ($outer.Left - 2) -and $inner.Right -le ($outer.Right + 2) -and
        $inner.Top -ge ($outer.Top - 2) -and $inner.Bottom -le ($outer.Bottom + 2)
}

function Get-ScopedRetryButtons($scope, $window, $errorBounds) {
    $scopeBounds = $scope.Current.BoundingRectangle
    if (-not (Test-BoundsContain $window.Current.BoundingRectangle $scopeBounds)) { return @() }
    $buttons = [System.Collections.Generic.List[object]]::new()
    foreach ($button in $scope.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)) {
        if ($button.Current.IsOffscreen -or -not $button.Current.IsEnabled -or
            -not (Test-RetryName $button.Current.Name)) { continue }
        $bounds = $button.Current.BoundingRectangle
        if (-not (Test-BoundsContain $scopeBounds $bounds)) { continue }
        if ($bounds.Top -lt ($errorBounds.Top - 32) -or $bounds.Top -gt ($errorBounds.Bottom + 240)) { continue }
        [void]$buttons.Add($button)
    }
    return $buttons.ToArray()
}

function Get-RetryCandidate {
    $candidates = [System.Collections.Generic.List[object]]::new()
    foreach ($process in @(Get-CodexDesktopProcesses)) {
        if ($process.MainWindowHandle -eq 0) { continue }
        try {
            $window = [System.Windows.Automation.AutomationElement]::FromHandle($process.MainWindowHandle)
            $windowBounds = $window.Current.BoundingRectangle
            $headings = @($window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $titleCondition) |
                Where-Object { -not $_.Current.IsOffscreen })
            if ($headings.Count -eq 0) { continue }
            $errors = @($window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $anyCondition) |
                Where-Object { -not $_.Current.IsOffscreen -and (Test-CapacityText $_.Current.Name) })
            foreach ($errorText in $errors) {
                $errorBounds = $errorText.Current.BoundingRectangle
                $scope = $errorText
                for ($depth = 1; $depth -le 10; $depth++) {
                    $scope = Get-CapacityParent $scope
                    if ($null -eq $scope -or $scope.Equals($window) -or
                        $scope.Current.ControlType -eq [System.Windows.Automation.ControlType]::Document) { break }
                    $hasTitle = @($scope.FindAll([System.Windows.Automation.TreeScope]::Descendants, $titleCondition) |
                        Where-Object { -not $_.Current.IsOffscreen }).Count -gt 0
                    if (-not $hasTitle) { continue }
                    foreach ($button in @(Get-ScopedRetryButtons $scope $window $errorBounds)) {
                        [void]$candidates.Add([PSCustomObject]@{
                            ProcessId = $process.Id
                            Button = $button
                            Key = [string]$process.Id + ':' + ($button.GetRuntimeId() -join '.')
                        })
                    }
                    if ($candidates.Count -gt 0) { break }
                }
            }
        } catch {
            $script:lastReason = $_.Exception.Message
        }
    }
    $unique = @{}
    foreach ($candidate in $candidates) { $unique[$candidate.Key] = $candidate }
    if ($unique.Count -eq 1) { return @($unique.Values)[0] }
    if ($unique.Count -gt 1) { $script:lastReason = 'Multiple capacity Retry buttons are visible.' }
    return $null
}

$deadline = [DateTime]::UtcNow.AddSeconds(12)
$previousKey = $null
while ([DateTime]::UtcNow -lt $deadline) {
    $candidate = Get-RetryCandidate
    if ($null -ne $candidate -and $previousKey -eq $candidate.Key) {
        [Console]::Out.WriteLine('READY')
        [Console]::Out.Flush()
        if ([Console]::In.ReadLine() -ne 'RETRY') { exit 0 }
        $verified = Get-RetryCandidate
        if ($null -eq $verified -or $verified.Key -ne $candidate.Key) {
            [Console]::Out.WriteLine('REVOKED')
            [Console]::Out.Flush()
            $previousKey = $null
            Start-Sleep -Milliseconds 500
            continue
        }
        $script:invokeAttempted = $true
        [Console]::Out.WriteLine('INVOKING')
        [Console]::Out.Flush()
        $invoke = $null
        if (-not $verified.Button.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$invoke)) {
            throw 'Retry button does not expose InvokePattern.'
        }
        $invoke.Invoke()
        [Console]::Out.WriteLine('RETRIED')
        [Console]::Out.Flush()
        exit 0
    }
    $previousKey = if ($null -eq $candidate) { $null } else { $candidate.Key }
    Start-Sleep -Milliseconds 1000
}
throw ('Timed out waiting for capacity Retry: ' + $script:lastReason)
