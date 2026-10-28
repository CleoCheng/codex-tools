$ErrorActionPreference = 'Stop'
$scriptPath = Join-Path $PSScriptRoot '../src-tauri/src/auto_switch_continue_windows.ps1'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Resolve-Path -LiteralPath $scriptPath).Path, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -ne 0) { throw ($parseErrors | Out-String) }

function Assert-True($condition, $message) {
    if (-not $condition) { throw $message }
}

# Reproduce the installed Store build's actual executable identity. The CLI and
# the ordinary ChatGPT package must not qualify just because their names match.
foreach ($functionName in @('Test-CodexDesktopProcess', 'Get-CodexDesktopProcesses', 'Test-StopControl', 'Test-RunningTransition', 'Wait-RunningTransition')) {
    $definition = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $functionName
    }, $true)
    . ([scriptblock]::Create($definition.Extent.Text))
}
$storeApp = 'C:\Program Files\WindowsApps\OpenAI.Codex_26.1016_x64__publisher\app\ChatGPT.exe'
Assert-True (Test-CodexDesktopProcess $storeApp '') 'The installed Codex package ChatGPT.exe must be detected.'
Assert-True (Test-CodexDesktopProcess 'C:/Program Files/WindowsApps/OpenAI.Codex_26_x64__publisher/app/Codex.exe' '') 'Legacy packaged Codex.exe must remain supported.'
Assert-True (-not (Test-CodexDesktopProcess 'C:\Program Files\WindowsApps\OpenAI.ChatGPT_1_x64__publisher\app\ChatGPT.exe' '')) 'Ordinary ChatGPT is not Codex.'
Assert-True (-not (Test-CodexDesktopProcess 'C:\Program Files\WindowsApps\OpenAI.Codex_26_x64__publisher\app\resources\codex.exe' '')) 'Codex CLI child is not a desktop.'
Assert-True (-not (Test-CodexDesktopProcess 'C:\Tools\Codex.exe' '')) 'An unrelated process must not qualify by name.'
Assert-True (Test-CodexDesktopProcess 'c:\custom\Codex.exe' 'C:\Custom\Codex.exe') 'The resolved configured desktop path must be accepted.'
function Get-Process {
    param([string[]]$Name, $ErrorAction)
    if ($Name -contains 'ChatGPT') { [pscustomobject]@{Path=$storeApp; Id=42; MainWindowHandle=99} }
    [pscustomobject]@{Path='C:\Tools\Codex.exe'; Id=43; MainWindowHandle=100}
}
$detected = @(Get-CodexDesktopProcesses)
Assert-True ($detected.Count -eq 1 -and $detected[0].Id -eq 42) 'Process discovery must query ChatGPT and retain the verified desktop, excluding CLI names.'
Remove-Item Function:Get-Process

# A visible Chromium draft may expose empty ValuePattern and nonempty TextPattern.
# Mock both patterns; this regression check never accesses a real window.
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName UIAutomationClient
# Construct the production filters without querying any live window. This
# catches PowerShell 5.1 overload/binding errors as well as parser errors.
foreach ($variableName in @('$editType', '$documentType', '$focusableCondition', '$editableDocument', '$editCondition', '$buttonCondition', '$enabledButtonCondition')) {
    $assignment = @($ast.EndBlock.Statements | Where-Object {
        $_ -is [System.Management.Automation.Language.AssignmentStatementAst] -and $_.Left.Extent.Text -eq $variableName
    })
    Assert-True ($assignment.Count -eq 1) ('Expected one production filter: ' + $variableName)
    . ([scriptblock]::Create($assignment[0].Extent.Text))
}
Assert-True ($enabledButtonCondition.GetConditions().Count -eq 3) 'Renderer readiness must select one visible enabled Button.'
foreach ($functionName in @('Normalize-ComposerText', 'Get-ComposerText', 'Get-SubmitControlKind', 'Get-NavigationAction')) {
    $definition = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $functionName
    }, $true)
    . ([scriptblock]::Create($definition.Extent.Text))
}
function New-FakeComposer($value, $text) {
    $range = [pscustomobject]@{Text=$text}
    $range | Add-Member ScriptMethod GetText { param($length) return $this.Text }
    $element = [pscustomobject]@{Value=$value; Range=$range}
    $element | Add-Member ScriptMethod TryGetCurrentPattern {
        param($patternId, $result)
        if ($patternId -eq [System.Windows.Automation.ValuePattern]::Pattern) {
            $result.Value = [pscustomobject]@{Current=[pscustomobject]@{Value=$this.Value}}
            return $true
        }
        $result.Value = [pscustomobject]@{DocumentRange=$this.Range}
        return $true
    }
    return $element
}
Add-Type -AssemblyName UIAutomationClient
$continue = [string][char]0x7ee7 + [char]0x7eed
Assert-True ((Get-ComposerText (New-FakeComposer '' ($continue + "`r`n"))) -eq $continue) 'Empty ValuePattern must fall back to the actual TextPattern draft.'
Assert-True ((Get-ComposerText (New-FakeComposer ([string][char]0x200b) ([string][char]0xfeff + $continue))) -eq $continue) 'Invisible editor markers must not hide the continue draft.'
Assert-True ((Get-ComposerText (New-FakeComposer 'Changed draft' $continue)) -eq 'Changed draft') 'A different nonempty draft must never qualify through the fallback.'
Assert-True ((Get-SubmitControlKind ([string][char]0x53d1 + [char]0x9001)) -eq 'Send') 'Localized Send must qualify.'
Assert-True ((Get-SubmitControlKind ([string][char]0x5f15 + [char]0x5bfc)) -eq 'Running') 'Steer must be recognized as a running control, never Send.'
Assert-True ((Get-SubmitControlKind 'Stop') -eq 'Running') 'Stop must never qualify as Send.'
Assert-True ((Get-SubmitControlKind 'Skip') -eq 'Other') 'An unrelated dialog button must not qualify.'

# The App can consume the initial deep link while restoring its empty Home.
# Navigation decisions depend on observed readiness, not the existence of a PID.
function New-NavigationState {
    return @{AppKey=''; RouteAttempts=0; PrefillAttempts=0; LastRouteMs=-10000L; LastPrefillMs=-10000L}
}
$nav = New-NavigationState
$startupObservation = [pscustomobject]@{AppReady=$false; AppForeground=$true; AppKey=''; TargetCount=0; Prefilled=$false; ConflictingDraft=$false}
Assert-True ((Get-NavigationAction $startupObservation $nav 0) -eq 'Wait' -and $nav.RouteAttempts -eq 0) 'Cold startup must not consume a navigation attempt.'
$startupObservation.AppReady = $true
$startupObservation.AppKey = '123:window'
Assert-True ((Get-NavigationAction $startupObservation $nav 400) -eq 'Wait') 'The loaded composer must be stable across observations.'
Assert-True ((Get-NavigationAction $startupObservation $nav 800) -eq 'Route') 'A stable empty Home must automatically navigate to the task.'
Assert-True ((Get-NavigationAction $startupObservation $nav 1200) -eq 'Wait') 'Navigation must not be spammed while the task loads.'
Assert-True ((Get-NavigationAction $startupObservation $nav 8800) -eq 'Route') 'A discarded startup navigation must be retried without a user click.'
$startupObservation.TargetCount = 1
Assert-True ((Get-NavigationAction $startupObservation $nav 9200) -eq 'Prefill') 'The verified empty task should be prefilled after navigation.'
$startupObservation.Prefilled = $true
Assert-True ((Get-NavigationAction $startupObservation $nav 20000) -eq 'Wait') 'A prefilled task must not be reopened or overwritten.'
$startupObservation.Prefilled = $false
$startupObservation.ConflictingDraft = $true
Assert-True ((Get-NavigationAction $startupObservation $nav 20000) -eq 'Wait') 'A different target draft must not be overwritten.'
$startupObservation.ConflictingDraft = $false
$startupObservation.TargetCount = 2
Assert-True ((Get-NavigationAction $startupObservation $nav 20000) -eq 'Wait') 'Ambiguous same-title composers must not be prefilled.'
$startupObservation.TargetCount = 0
$nav.RouteAttempts = 6
Assert-True ((Get-NavigationAction $startupObservation $nav 100000) -eq 'Wait') 'Navigation retries must be bounded.'
$startupObservation.TargetCount = 1
$nav.PrefillAttempts = 3
Assert-True ((Get-NavigationAction $startupObservation $nav 100000) -eq 'Wait') 'Prefill retries must be bounded separately from routing.'

# Once acquired/prefilled, a transient missing title/editor or user navigation
# must not dispatch another deep link, including after returning to the app.
$nav = New-NavigationState
$nav.AppKey = '123:window'
$startupObservation.TargetCount = 1
Assert-True ((Get-NavigationAction $startupObservation $nav 0) -eq 'Prefill') 'An acquired task may be prefilled once.'
$startupObservation.TargetCount = 0
$startupObservation.Prefilled = $false
Assert-True ((Get-NavigationAction $startupObservation $nav 20000) -eq 'Wait' -and $nav.RouteAttempts -eq 0 -and $nav.PrefillAttempts -eq 1) 'A remounted or user-hidden composer must never be reopened after prefill.'
$startupObservation.TargetCount = 1
Assert-True ((Get-NavigationAction $startupObservation $nav 30000) -eq 'Wait' -and $nav.PrefillAttempts -eq 1) 'A temporarily empty draft must not trigger a second prefill.'
$nav = New-NavigationState
$nav.AppKey = '123:window'
$startupObservation.TargetCount = 0
$startupObservation.AppForeground = $false
Assert-True ((Get-NavigationAction $startupObservation $nav 40000) -eq 'Wait' -and $nav.RouteAttempts -eq 0) 'A background app must never open a route or steal focus.'
$startupObservation.TargetCount = 1
Assert-True ((Get-NavigationAction $startupObservation $nav 41000) -eq 'Wait' -and $nav.PrefillAttempts -eq 0) 'A background target must never be prefilled.'
$startupObservation.TargetCount = 0
$startupObservation.AppForeground = $true
Assert-True ((Get-NavigationAction $startupObservation $nav 50000) -eq 'Wait' -and $nav.RouteAttempts -eq 0) 'Once acquired in the background, a missing target must not be reopened later.'

# Evaluate only the pure geometry function; never load UIAutomation or navigate the installed App.
$geometry = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Test-ContentTitleBounds'
}, $true)
. ([scriptblock]::Create($geometry.Extent.Text))
function New-Bounds($left, $top, $right, $bottom) {
    return [pscustomobject]@{Left=$left; Top=$top; Right=$right; Bottom=$bottom; IsEmpty=$false}
}
$window = New-Bounds 0 0 1800 1000
$composer = New-Bounds 600 800 1500 900
$title = New-Bounds 300 40 500 70
$main = New-Bounds 280 0 1800 1000
$sidebar = New-Bounds 20 40 250 70
Assert-True (-not (Test-ContentTitleBounds $title $composer $null $window $false)) 'Wide-window fixture must require a content ancestor.'
Assert-True (Test-ContentTitleBounds $title $composer $main $window $false) 'Left-aligned header should qualify in a wide content panel.'
Assert-True (-not (Test-ContentTitleBounds $sidebar $composer $main $window $false)) 'Sidebar title must not identify the current conversation.'
Assert-True (-not (Test-ContentTitleBounds $title $composer $window $window $false)) 'A common whole-window ancestor must not qualify as main content.'
Assert-True (Test-ContentTitleBounds $title $composer $window $window $true) 'The explicit main landmark should support a collapsed sidebar.'
Assert-True (-not (Test-ContentTitleBounds (New-Bounds 650 920 800 950) $composer $main $window $true)) 'Transcript text below the composer must not count as the title.'

# Tabbed single-line composers may put their footer six accessible ancestors
# away and reserve space to the right of the text editor. Exercise the actual
# production scope traversal with fake elements only; no live App UI is read.
foreach ($functionName in @('Test-ComposerScopeBounds','Test-ComposerControlBounds','Get-ComposerControls')) {
    $definition = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $functionName
    }, $true)
    . ([scriptblock]::Create($definition.Extent.Text))
}
$inline = New-Bounds 600 800 1200 824
$frame = New-Bounds 580 786 1480 846
$sendBounds = New-Bounds 1420 792 1452 824
Assert-True (Test-ComposerControlBounds $sendBounds $inline $frame) 'Inline sibling Send above the text baseline and outside the text width must qualify within its composer frame.'
Assert-True (-not (Test-ComposerControlBounds (New-Bounds 1580 792 1612 824) $inline $frame)) 'Browser-tab controls outside the composer frame must not qualify.'
Assert-True (-not (Test-ComposerControlBounds (New-Bounds 1420 600 1452 632) $inline $frame)) 'Transcript controls must not qualify.'
Assert-True (-not (Test-ComposerScopeBounds $window $inline)) 'The whole application must not qualify as a compact composer frame.'

function New-FakeElement($bounds, $kind, $name) {
    $element = [pscustomobject]@{Current=[pscustomobject]@{
        BoundingRectangle=$bounds; ControlType=$kind; Name=$name; IsOffscreen=$false; IsEnabled=$true; ClassName=''
    }; Parent=$null; RawParent=$null; Buttons=@(); Edits=@()}
    $element | Add-Member ScriptMethod GetRuntimeId { return @($this.Current.Name) }
    $element | Add-Member ScriptMethod FindAll {
        param($scope,$condition)
        if ($condition -eq 'fake-edits') { return $this.Edits }
        return $this.Buttons
    }
    return $element
}
function Get-ContinueParent($element, [switch]$Raw) { if ($Raw -and $element.RawParent) { return $element.RawParent }; return $element.Parent }
$buttonCondition = 'fake-buttons'
$editCondition = 'fake-edits'
$fakeWindow = New-FakeElement $window ([System.Windows.Automation.ControlType]::Window) 'window'
$edit = New-FakeElement $inline ([System.Windows.Automation.ControlType]::Edit) 'composer'
$edit.Current.ClassName = 'ProseMirror'
$button = New-FakeElement $sendBounds ([System.Windows.Automation.ControlType]::Button) 'Send'
$child = $edit
$ancestors = @()
for ($level=1; $level -le 6; $level++) {
    $bounds = if ($level -eq 6) { $frame } else { $inline }
    $parent = New-FakeElement $bounds ([System.Windows.Automation.ControlType]::Group) ('scope-' + $level)
    $parent.Edits = @($edit)
    $child.Parent = $parent
    $child = $parent
    $ancestors += $parent
}
$child.Parent = $fakeWindow
$child.Buttons = @($button)
$found = @(Get-ComposerControls $edit $fakeWindow)
Assert-True ($found.Count -eq 1 -and $found[0].Depth -eq 6 -and $found[0].Kind -eq 'Send') 'A tabbed sibling Send must be found beyond the old four-ancestor limit.'
$button.Current.Name = 'Stop'
$found = @(Get-ComposerControls $edit $fakeWindow)
Assert-True ($found.Count -eq 1 -and $found[0].Kind -eq 'Running') 'The same frame must detect Stop after submission.'
$button.Current.Name = 'Send'
$button.Current.IsOffscreen = $true
Assert-True (@(Get-ComposerControls $edit $fakeWindow).Count -eq 0) 'A hidden background tab Send must not qualify.'
$button.Current.IsOffscreen = $false
$otherEdit = New-FakeElement $inline ([System.Windows.Automation.ControlType]::Edit) 'other-composer'
$otherEdit.Current.ClassName = 'ProseMirror'
$child.Edits = @($edit,$otherEdit)
Assert-True (@(Get-ComposerControls $edit $fakeWindow).Count -eq 0) 'A frame containing another visible rich-text composer must be rejected.'
$child.Edits = @($edit)
$near = $ancestors[1]
$near.Current.BoundingRectangle = $frame
$near.Buttons = @($button)
$found = @(Get-ComposerControls $edit $fakeWindow)
Assert-True ($found.Count -eq 1 -and $found[0].Depth -eq 2) 'Normal chat should keep using the nearest valid composer frame.'
$near.Buttons = @()
$child.Current.ControlType = [System.Windows.Automation.ControlType]::Document
Assert-True (@(Get-ComposerControls $edit $fakeWindow).Count -eq 0) 'Traversal must stop before an embedded browser Document.'
# Control View can skip wrappers that Raw View retains after a re-render.
$child.Current.ControlType = [System.Windows.Automation.ControlType]::Group
$edit.Parent = $fakeWindow
$edit.RawParent = $ancestors[0]
$found = @(Get-ComposerControls $edit $fakeWindow)
Assert-True ($found.Count -eq 1 -and $found[0].Depth -eq 6 -and $script:scopeDiagnostics.Raw -eq 1) 'Raw View must recover a compact footer omitted by Control View.'
$button.Current.BoundingRectangle = New-Bounds 1600 792 1632 824
Assert-True (@(Get-ComposerControls $edit $fakeWindow).Count -eq 0 -and $script:scopeDiagnostics.Outside -gt 0) 'Raw fallback must still exclude another panel button and diagnose the exclusion.'
$button.Current.BoundingRectangle = $sendBounds
$child.Edits = @($edit,$otherEdit)
Assert-True (@(Get-ComposerControls $edit $fakeWindow).Count -eq 0 -and $script:scopeDiagnostics.OtherEditors -gt 0) 'Raw fallback must retain the second-composer boundary.'
$child.Edits = @($edit)
$child.Current.ControlType = [System.Windows.Automation.ControlType]::Document
Assert-True (@(Get-ComposerControls $edit $fakeWindow).Count -eq 0) 'Raw fallback must stop at the same Document boundary.'
Assert-True (Test-ComposerScopeBounds (New-Bounds 601 801 1200 824) $inline) 'A one-pixel animated bound mismatch must not reject the composer.'
Remove-Item Function:Get-ContinueParent

# Candidate identity must remain unique across windows and side-chat composers.
# Evaluate this pure selector without discovering windows or invoking any control.
$selector = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Select-UniqueReadyCandidate'
}, $true)
. ([scriptblock]::Create($selector.Extent.Text))
$first = [pscustomobject]@{Key='123:composer-1'; Invoke='first'}
$duplicate = [pscustomobject]@{Key='123:composer-1'; Invoke='same composer'}
$secondWindow = [pscustomobject]@{Key='456:composer-2'; Invoke='different window'}
$sideChat = [pscustomobject]@{Key='123:composer-3'; Invoke='different composer'}
$sameComposerOne = [pscustomobject]@{Key='123:composer-1'; ControlKey='123:composer-1:button-a'}
$sameComposerTwo = [pscustomobject]@{Key='123:composer-1'; ControlKey='123:composer-1:button-b'}
Assert-True ($null -eq (Select-UniqueReadyCandidate @($sameComposerOne,$sameComposerTwo))) 'Two distinct Send controls in the same composer must remain ambiguous.'
Assert-True ($null -eq (Select-UniqueReadyCandidate @())) 'No matching composer must remain unready.'
Assert-True ((Select-UniqueReadyCandidate @($first)).Key -eq $first.Key) 'One composer must qualify.'
Assert-True ((Select-UniqueReadyCandidate @($first, $duplicate)).Key -eq $first.Key) 'Repeated Send hits for the same composer must be deduplicated.'
Assert-True ($null -eq (Select-UniqueReadyCandidate @($first, $secondWindow))) 'Two matching windows must not select the first draft.'
Assert-True ($null -eq (Select-UniqueReadyCandidate @($first, $sideChat))) 'Two matching composers in one process must be ambiguous.'

# Run only the parsed readiness/handshake loop in isolated processes with fake candidates.
# None of the production initializers, native APIs or Get-ReadyCandidate implementation execute.
$loop = @($ast.EndBlock.Statements | Where-Object {
    $_ -is [System.Management.Automation.Language.WhileStatementAst]
})
Assert-True ($loop.Count -eq 1) 'Expected one top-level readiness loop.'
$loopText = $loop[0].Extent.Text
$trapText = @($ast.EndBlock.Traps)[0].Extent.Text
$timeoutText = @($ast.EndBlock.Statements | Where-Object {
    $_ -is [System.Management.Automation.Language.ThrowStatementAst]
})[-1].Extent.Text
$powerShell = (Get-Command powershell.exe).Source

function Run-Fixture($reply, $changed, $invokeThrows, $neverReady, $buttonChanged = $false, $transient = $false) {
    $fixture = @'
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$counter = 0
$invoke = [pscustomobject]@{}
$invoke | Add-Member -MemberType ScriptMethod -Name Invoke -Value {
    [Console]::Out.WriteLine('INVOKED')
    if (INVOKE_THROWS) { throw 'Simulated uncertain invocation' }
}
$candidate = [pscustomobject]@{Key='target'; ControlKey='button-a'; Invoke=$invoke}
function Get-ReadyCandidate {
    $script:counter++
    if ($script:counter -gt 12) { $script:deadline = [DateTime]::UtcNow.AddSeconds(-1) }
    if (NEVER_READY) { return $null }
    if (BUTTON_CHANGED -and $script:counter -gt 1 -and (-not TRANSIENT -or $script:counter -eq 2)) { return [pscustomobject]@{Key='target'; ControlKey='button-b'; Invoke=$invoke} }
    if (CHANGED -and $script:counter -gt 1 -and (-not TRANSIENT -or $script:counter -eq 2)) { return [pscustomobject]@{Key='other'; Invoke=$invoke} }
    return $candidate
}
function Update-ConversationNavigation {}
function Write-Stage($stage) { [Console]::Out.WriteLine('TRACE stage=' + $stage) }
function Start-Sleep { param($Milliseconds) [Threading.Thread]::Sleep(5) }
function Wait-RunningTransition($candidate) { return $false }
$deadline = [DateTime]::UtcNow.AddSeconds(1)
$previousKey = 'target:button-a'
$script:lastReason = 'waiting_for_enabled_send_control'
'@
    $fixture = $fixture.Replace('INVOKE_THROWS', ('$' + $invokeThrows.ToString().ToLowerInvariant()))
    $fixture = $fixture.Replace('NEVER_READY', ('$' + $neverReady.ToString().ToLowerInvariant()))
    $fixture = $fixture.Replace('BUTTON_CHANGED', ('$' + $buttonChanged.ToString().ToLowerInvariant()))
    $fixture = $fixture.Replace('TRANSIENT', ('$' + $transient.ToString().ToLowerInvariant()))
    $fixture = $fixture.Replace('CHANGED', ('$' + $changed.ToString().ToLowerInvariant()))
    $fixture += [Environment]::NewLine + $trapText + [Environment]::NewLine + $loopText + [Environment]::NewLine + $timeoutText
    return (Run-IsolatedFixture $fixture $reply)
}

function Run-IsolatedFixture($fixture, $reply) {
    # Exercise the production environment-script bootstrap, keeping the command
    # line bounded even when diagnostics make the embedded script larger.
    $bootstrap = '& ([scriptblock]::Create($env:CODEX_TOOLS_CONTINUE_SCRIPT))'
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($bootstrap))
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $powerShell
    $info.Arguments = '-NoLogo -NoProfile -NonInteractive -EncodedCommand ' + $encoded
    $info.EnvironmentVariables['CODEX_TOOLS_CONTINUE_SCRIPT'] = $fixture
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $info
    [void]$process.Start()
    $process.StandardInput.WriteLine($reply)
    $process.StandardInput.Close()
    $output = $process.StandardOutput.ReadToEnd()
    $errors = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    $result = [pscustomobject]@{Output=$output; Errors=$errors; ExitCode=$process.ExitCode}
    $process.Dispose()
    return $result
}

$cancel = Run-Fixture 'CANCEL' $false $false $false
Assert-True ($cancel.ExitCode -eq 0 -and $cancel.Output -match 'READY' -and $cancel.Output -notmatch 'INVOKED|SUBMITTED') 'Cancelled/completed tasks must never submit.'
$submit = Run-Fixture 'SUBMIT' $false $false $false
Assert-True ($submit.ExitCode -eq 0 -and $submit.Output -match 'SUBMITTED' -and @($submit.Output -split "\r?\n" | Where-Object {$_ -eq 'INVOKED'}).Count -eq 1) 'A verified task must be invoked exactly once.'
$changed = Run-Fixture "SUBMIT`nCANCEL" $true $false $false
Assert-True ($changed.ExitCode -eq 0 -and $changed.Output -match 'REVOKED' -and $changed.Output -notmatch 'INVOKING|INVOKED|SUBMITTED') 'A changed composer must revoke readiness and require fresh parent authorization.'
$changedButton = Run-Fixture "SUBMIT`nCANCEL" $false $false $false $true
Assert-True ($changedButton.ExitCode -eq 0 -and $changedButton.Output -match 'REVOKED' -and $changedButton.Output -notmatch 'INVOKING|INVOKED|SUBMITTED') 'The same composer with a replacement Send control must revoke readiness.'
$recovered = Run-Fixture "SUBMIT`nSUBMIT" $false $false $false $true $true
Assert-True ($recovered.ExitCode -eq 0 -and $recovered.Output -match 'REVOKED[\s\S]*READY[\s\S]*INVOKING[\s\S]*INVOKED[\s\S]*SUBMITTED' -and @($recovered.Output -split "\r?\n" | Where-Object {$_ -eq 'INVOKED'}).Count -eq 1) 'A goal/tab re-render before Invoke must recover through fresh readiness and submit only once.'
$uncertain = Run-Fixture 'SUBMIT' $false $true $false
Assert-True ($uncertain.Output -match 'INVOKING' -and $uncertain.Output -notmatch 'NOT_SUBMITTED|REVOKED' -and $uncertain.ExitCode -ne 0 -and @($uncertain.Output -split "\r?\n" | Where-Object {$_ -eq 'INVOKED'}).Count -eq 1 -and $uncertain.Output -notmatch 'SUBMITTED') 'An uncertain invocation must not be retried.'
$loading = Run-Fixture 'SUBMIT' $false $false $true
Assert-True ($loading.Output -match 'NOT_SUBMITTED' -and $loading.Output -notmatch 'READY|INVOKING|INVOKED') 'An unloaded task must never accept permission to submit.'
Assert-True ($loading.ExitCode -ne 0 -and $loading.Errors -match 'Timed out.*waiting_for_enabled_send_control' -and $loading.Errors -notmatch 'CLIXML|<Objs') 'Readiness timeout must report the actual stage as a plain-text error.'

# Run the real navigation/update/handshake loop with a fake Home and link opener.
# No real process lookup, native link, UI access, manual click or user message.
function Run-HomeFixture($discardFirstRoute, $differentDraft) {
    $definitions = foreach ($functionName in @('Write-Stage', 'Get-NavigationAction', 'Update-ConversationNavigation')) {
        $definition = $ast.Find({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $functionName
        }, $true)
        if ($functionName -eq 'Get-NavigationAction') {
            $definition.Extent.Text.Replace('function Get-NavigationAction(', 'function Get-ProductionNavigationAction(')
        } else { $definition.Extent.Text }
    }
    $fixture = @'
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$navigationUri = 'codex://threads/019a0123-4567-7abc-8def-0123456789ab'
$uri = $navigationUri + '?prompt=continue-fixture'
$script:navigationState = @{AppKey=''; RouteAttempts=0; PrefillAttempts=0; LastRouteMs=-10000L; LastPrefillMs=-10000L}
$polls = 0
$routes = 0
$targetVisible = $false
$prefilled = $false
$invoke = [pscustomobject]@{}
$invoke | Add-Member ScriptMethod Invoke { [Console]::Out.WriteLine('INVOKED') }
function Get-NavigationAction($observation, $state, [long]$nowMs) {
    # Advance the injected decision clock instead of waiting eight real seconds.
    return (Get-ProductionNavigationAction $observation $state ($script:polls * 9000))
}
function Open-ConversationLink($link) {
    if ($script:polls -lt 3) { throw 'Navigation happened before the App was ready.' }
    if ($link -eq $navigationUri) {
        $script:routes++
        [Console]::Out.WriteLine('ROUTE')
        if (-not (DISCARD_FIRST -and $script:routes -eq 1)) { $script:targetVisible = $true }
    } elseif ($link -eq $uri) {
        if (-not $script:targetVisible -or DIFFERENT_DRAFT) { throw 'Prefilled an unidentified task or overwrote a different draft.' }
        $script:prefilled = $true
        [Console]::Out.WriteLine('PREFILL')
    } else { throw 'Unexpected conversation link.' }
}
function Get-ReadyCandidate {
    $script:polls++
    $script:navigationObservation = [pscustomobject]@{
        AppReady = $script:polls -ge 3
        AppForeground = $true
        AppKey = '123:fixture-window'
        TargetCount = [int]$script:targetVisible
        Prefilled = $script:prefilled
        ConflictingDraft = $script:targetVisible -and DIFFERENT_DRAFT
    }
    if ($script:polls -gt 15) { $script:deadline = [DateTime]::UtcNow.AddSeconds(-1) }
    if ($script:prefilled) { return [pscustomobject]@{Key='target'; Invoke=$invoke} }
    return $null
}
function Wait-RunningTransition($candidate) { return $false }
function Start-Sleep { param($Milliseconds) [Threading.Thread]::Sleep(5) }
$script:deadline = [DateTime]::UtcNow.AddSeconds(3)
$previousKey = $null
$script:lastReason = 'target_has_different_draft'
'@
    $fixture = $fixture.Replace('DISCARD_FIRST', ('$' + $discardFirstRoute.ToString().ToLowerInvariant()))
    $fixture = $fixture.Replace('DIFFERENT_DRAFT', ('$' + $differentDraft.ToString().ToLowerInvariant()))
    $fixture += "`n" + ($definitions -join "`n") + "`n" + $trapText + "`n" + $loopText + "`n" + $timeoutText
    return (Run-IsolatedFixture $fixture 'SUBMIT')
}

$fromHome = Run-HomeFixture $false $false
Assert-True ($fromHome.ExitCode -eq 0 -and $fromHome.Output -match 'ROUTE[\s\S]*PREFILL[\s\S]*READY[\s\S]*INVOKED[\s\S]*SUBMITTED') ('Empty Home must navigate, prefill and submit automatically in order. Fixture: ' + ($fromHome | ConvertTo-Json -Compress))
$discardedRoute = Run-HomeFixture $true $false
Assert-True ($discardedRoute.ExitCode -eq 0 -and @($discardedRoute.Output -split "\r?\n" | Where-Object {$_ -eq 'ROUTE'}).Count -eq 2 -and @($discardedRoute.Output -split "\r?\n" | Where-Object {$_ -eq 'INVOKED'}).Count -eq 1) 'A lost route must recover without a manual click and still submit exactly once.'
$differentDraft = Run-HomeFixture $false $true
Assert-True ($differentDraft.ExitCode -ne 0 -and $differentDraft.Output -match 'ROUTE' -and $differentDraft.Output -notmatch 'PREFILL|INVOKING|INVOKED|(?m)^SUBMITTED\r?$') 'Navigation into a task with a different draft must never overwrite or send it.'
# Post-submit observation is read-only and must follow Send -> Stop in the
# identical target composer, with two stable observations. No real UI is used.
$sent = [pscustomobject]@{Key='target'; Kind='Send'}
$stopped = [pscustomobject]@{Key='target'; Kind='Stop'}
Assert-True (Test-StopControl 'Stop response') 'Stop response must be recognized.'
Assert-True (Test-StopControl ([string][char]0x505c + [char]0x6b62)) 'Chinese Stop must be recognized.'
Assert-True (-not (Test-StopControl 'Steer')) 'Steer alone must not prove the Send -> Stop transition.'
Assert-True (Test-RunningTransition $sent $stopped) 'Same-composer Send -> Stop proves acceptance.'
Assert-True (-not (Test-RunningTransition $stopped $stopped)) 'A pre-existing Stop is not a submission transition.'
Assert-True (-not (Test-RunningTransition $sent ([pscustomobject]@{Key='other'; Kind='Stop'}))) 'A different composer Stop must not confirm this send.'
Assert-True (-not (Test-RunningTransition $sent $null)) 'An unavailable composer must fall back to log verification.'
$script:runningPolls = 0
function Get-ReadyCandidate([switch]$ObserveRunning) {
    Assert-True $ObserveRunning 'Post-submit polling must only observe running controls.'
    $script:runningPolls++
    if ($script:runningPolls -eq 1) { return [pscustomobject]@{Key='other';Kind='Stop'} }
    return [pscustomobject]@{Key='target';Kind='Stop'}
}
function Start-Sleep { param($Milliseconds) }
Assert-True (Wait-RunningTransition $sent) 'A stable matching Stop must confirm the submitted target.'
Assert-True ($script:runningPolls -eq 3) 'A foreign Stop must be ignored before two matching observations.'
Write-Output 'PASS: tabbed/inline composer scope, send-to-stop acceptance, PowerShell syntax, composer text fallback, Send/Steer classification, geometry, unique composer selection, foreground gating, one-way navigation, bounded Raw View recovery and ten isolated continuation/Home/revocation fixtures.'
