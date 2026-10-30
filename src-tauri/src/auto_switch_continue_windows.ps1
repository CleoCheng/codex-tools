$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$script:invokeAttempted = $false
trap {
    if (-not $script:invokeAttempted) {
        [Console]::Out.WriteLine('NOT_SUBMITTED')
        [Console]::Out.Flush()
    }
    # EncodedCommand otherwise serializes exceptions/progress as CLIXML and
    # hides the useful failure behind hundreds of characters of markup.
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class CodexToolsContinueNative {
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("shell32.dll", CharSet=CharSet.Unicode)]
    public static extern IntPtr ShellExecute(IntPtr hwnd, string operation, string file, string parameters, string directory, int show);
}
'@

$uri = $env:CODEX_TOOLS_CONTINUE_URI
$title = $env:CODEX_TOOLS_CONTINUE_TITLE
$prompt = $env:CODEX_TOOLS_CONTINUE_PROMPT
if ([string]::IsNullOrWhiteSpace($uri) -or [string]::IsNullOrWhiteSpace($title) -or
    [string]::IsNullOrWhiteSpace($prompt)) {
    throw 'Missing the target conversation identity.'
}
# A cold-start deep link can be consumed before the renderer restores Home.
# Dispatch only after a stable live App shell is observed, then verify navigation.
$navigationUri = $uri.Split('?')[0]
$script:navigationState = @{AppKey=''; RouteAttempts=0; PrefillAttempts=0; LastRouteMs=-10000L; LastPrefillMs=-10000L; TargetAcquired=$false; PrefillDispatched=$false}

function Normalize-ComposerText($text) {
    if ($null -eq $text) { return '' }
    return ([string]$text -replace '[\u200B\uFEFF]', '').Trim()
}

function Get-ComposerText($element) {
    $pattern = $null
    if ($element.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
        $value = Normalize-ComposerText $pattern.Current.Value
        if ($value.Length -gt 0) { return $value }
    }
    # Chromium contenteditable can expose ValuePattern with an empty Value while
    # the actual draft is available through TextPattern. Do not return early.
    $pattern = $null
    if ($element.TryGetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern, [ref]$pattern)) {
        return (Normalize-ComposerText $pattern.DocumentRange.GetText(-1))
    }
    return ''
}

function Test-ContentTitleBounds($titleBounds, $composerBounds, $scopeBounds, $windowBounds, $isMainScope) {
    if ($titleBounds.IsEmpty -or $composerBounds.IsEmpty -or $titleBounds.Bottom -ge $composerBounds.Top) {
        return $false
    }
    if ($null -eq $scopeBounds) {
        return $titleBounds.Right -gt $composerBounds.Left -and $titleBounds.Left -lt $composerBounds.Right
    }
    # A main-content ancestor supports wide windows where the header is left-aligned and the composer is centered.
    return -not $scopeBounds.IsEmpty -and $titleBounds.Left -ge $scopeBounds.Left -and
        $titleBounds.Right -le $scopeBounds.Right -and $titleBounds.Top -ge $scopeBounds.Top -and
        ($isMainScope -or $scopeBounds.Left -ge ($windowBounds.Left + 140))
}

function Test-MainContentTitle($heading, $edit, $window) {
    $titleBounds = $heading.Current.BoundingRectangle
    $composerBounds = $edit.Current.BoundingRectangle
    $windowBounds = $window.Current.BoundingRectangle
    if ($heading.Current.IsOffscreen) { return $false }
    if (Test-ContentTitleBounds $titleBounds $composerBounds $null $windowBounds $false) { return $true }
    $scope = $edit
    for ($level = 0; $level -lt 12; $level++) {
        $scope = [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetParent($scope)
        if ($null -eq $scope -or $scope.Equals($window) -or
            $scope.Current.ControlType -eq [System.Windows.Automation.ControlType]::Document) { break }
        # The installed App renders this panel with role="main".
        $isMain = $scope.Current.LocalizedControlType -match '^(main|main landmark|main content|\u4e3b\u533a\u57df|\u4e3b\u5185\u5bb9|\u4e3b\u8981\u5167\u5bb9|\u4e3b\u8981\u5185\u5bb9)$'
        if (Test-ContentTitleBounds $titleBounds $composerBounds $scope.Current.BoundingRectangle $windowBounds $isMain) {
            return $true
        }
    }
    return $false
}

$editType = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Edit)
$documentType = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Document)
$focusableCondition = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::IsKeyboardFocusableProperty, $true)
$editableDocument = [System.Windows.Automation.AndCondition]::new($documentType, $focusableCondition)
$editCondition = [System.Windows.Automation.OrCondition]::new($editType, $editableDocument)
$buttonCondition = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Button)
$enabledButtonCondition = [System.Windows.Automation.AndCondition]::new(
    $buttonCondition,
    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::IsEnabledProperty, $true),
    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::IsOffscreenProperty, $false))
$titleCondition = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::NameProperty, $title)
$script:lastReason = 'Waiting for the Codex conversation window.'

function Test-CodexDesktopProcess($processPath, $expectedPath) {
    if ([string]::IsNullOrWhiteSpace($processPath)) { return $false }
    $normalized = $processPath.Replace('/', '\')
    if (-not [string]::IsNullOrWhiteSpace($expectedPath) -and
        [string]::Equals($normalized, $expectedPath.Replace('/', '\'), [StringComparison]::OrdinalIgnoreCase)) {
        return $true
    }
    # The Codex Store package currently ships ChatGPT.exe. Exclude the ordinary
    # OpenAI.ChatGPT package and the codex.exe CLI child under app/resources.
    return $normalized -match '(?i)\\WindowsApps\\OpenAI\.Codex_[^\\]+\\app\\(ChatGPT|Codex|Codex Desktop)\.exe$'
}

function Get-CodexDesktopProcesses {
    foreach ($process in @(Get-Process -Name 'Codex', 'ChatGPT', 'Codex Desktop' -ErrorAction SilentlyContinue)) {
        try {
            if (Test-CodexDesktopProcess $process.Path $env:CODEX_TOOLS_CONTINUE_APP_PATH) { $process }
        } catch { }
    }
}

function Write-Stage($stage) {
    $script:lastReason = $stage
    if ($script:lastStage -ne $stage) {
        $script:lastStage = $stage
        [Console]::Out.WriteLine('TRACE stage=' + $stage)
        [Console]::Out.Flush()
    }
}

function Select-UniqueReadyCandidate([object[]]$candidates) {
    $unique = @{}
    foreach ($candidate in $candidates) {
        # Ancestor scopes can expose the same composer and Send button several times.
        $identity = if ($candidate.ControlKey) { $candidate.ControlKey } else { $candidate.Key }
        $unique[$identity] = $candidate
    }
    if ($unique.Count -eq 1) { return @($unique.Values)[0] }
    if ($unique.Count -gt 1) {
        $script:lastReason = 'Multiple matching conversation composers are visible; waiting for an unambiguous target.'
    }
    return $null
}

function Test-StopControl($name) {
    return $name -match '^(Stop|Stop response|Stop generating|\u505c\u6b62|\u505c\u6b62\u751f\u6210|\u505c\u6b62\u54cd\u5e94)(\s*[\(\uff08].*)?$'
}

function Get-SubmitControlKind($name) {
    if (Test-StopControl $name) { return 'Running' }
    if ($name -match '^(Send|Send message|Send prompt|Send follow-up|\u53d1\u9001|\u53d1\u9001\u6d88\u606f)(\s*[\(\uff08].*)?$') { return 'Send' }
    if ($name -match '^(Steer|Queue|Stop|Stop response|\u5f15\u5bfc|\u6392\u961f|\u505c\u6b62)(\s*[\(\uff08].*)?$') { return 'Running' }
    return 'Other'
}

function Get-ContinueParent($element, [switch]$Raw) {
    $walker = if ($Raw) { [System.Windows.Automation.TreeWalker]::RawViewWalker }
        else { [System.Windows.Automation.TreeWalker]::ControlViewWalker }
    return $walker.GetParent($element)
}

function Test-ComposerLayoutClass($className) {
    # Stable component names read from the installed App bundle; the generated
    # CSS hash may change between releases. Never match a whole chat/main pane.
    return $className -match '(^|\s)_?(ComposerLayoutRoot|ComposerLayoutBody|ComposerLayoutFooter|AnimatedAdaptiveFooter|DocumentComposerControls)(_|\s|$)'
}

function Test-ComposerLayoutBounds($scope, $composer, $window) {
    if ($null -eq $scope -or $scope.IsEmpty -or $composer.IsEmpty -or $window.IsEmpty) { return $false }
    return $scope.Left -le ($composer.Left + 2) -and $scope.Right -ge ($composer.Right - 2) -and
        $scope.Top -le ($composer.Top + 2) -and $scope.Bottom -ge ($composer.Bottom - 2) -and
        $scope.Left -ge ($window.Left - 2) -and $scope.Right -le ($window.Right + 2) -and
        $scope.Top -ge ($window.Top - 2) -and $scope.Bottom -le ($window.Bottom + 2) -and
        ($scope.Bottom - $scope.Top) -le [Math]::Max(600, ($composer.Bottom - $composer.Top) + 320)
}

function Test-ComposerScopeBounds($scope, $composer) {
    # A tab may add wrappers between the editable text and its sibling toolbar.
    # Never expand the search into the transcript, browser, sidebar or whole app.
    if ($null -eq $scope -or $scope.IsEmpty -or $composer.IsEmpty) { return $false }
    # Chromium animated/subpixel bounds can differ by a physical pixel.
    return $scope.Left -le ($composer.Left + 2) -and $scope.Right -ge ($composer.Right - 2) -and
        $scope.Top -le ($composer.Top + 2) -and $scope.Bottom -ge ($composer.Bottom - 2) -and
        ($scope.Right - $scope.Left) -le (($composer.Right - $composer.Left) + 384) -and
        ($scope.Bottom - $scope.Top) -le (($composer.Bottom - $composer.Top) + 200)
}

function Test-ComposerControlBounds($button, $composer, $scope, [switch]$Layout) {
    if ($button.IsEmpty -or $scope.IsEmpty -or
        (-not $Layout -and -not (Test-ComposerScopeBounds $scope $composer))) { return $false }
    if ($Layout) {
        # A proven composer component owns the sibling footer. Its control may
        # be farther from the editable text in goal/attachment/scaled layouts.
        return $button.Left -ge ($scope.Left - 2) -and $button.Right -le ($scope.Right + 2) -and
            $button.Top -ge ($scope.Top - 2) -and $button.Bottom -le ($scope.Bottom + 2) -and
            $button.Top -ge ($composer.Top - 24) -and
            ($button.Right - $button.Left) -le 144 -and ($button.Bottom - $button.Top) -le 144
    }
    return $button.Left -ge ($scope.Left - 2) -and $button.Right -le ($scope.Right + 2) -and
        $button.Top -ge ($scope.Top - 2) -and $button.Bottom -le ($scope.Bottom + 2) -and
        $button.Left -ge ($composer.Left - 64) -and $button.Right -le ($composer.Right + 384) -and
        $button.Top -ge ($composer.Top - 24) -and $button.Bottom -le ($composer.Bottom + 120) -and
        ($button.Right - $button.Left) -le 96 -and ($button.Bottom - $button.Top) -le 96
}

function Get-ComposerControls($edit, $window, [switch]$Raw) {
    if (-not $Raw) { $script:scopeDiagnostics = @{Scopes=0; RejectedBounds=0; OtherEditors=0; Buttons=0; Offscreen=0; Unknown=0; Outside=0; Boundary=0; Raw=0; Layout=0} }
    else { $script:scopeDiagnostics.Raw++ }
    $composerBounds = $edit.Current.BoundingRectangle
    $scope = $edit
    $maxDepth = if ($Raw) { 24 } else { 12 }
    for ($depth = 1; $depth -le $maxDepth; $depth++) {
        $scope = Get-ContinueParent $scope -Raw:$Raw
        if ($null -eq $scope -or $scope.Equals($window) -or
            $scope.Current.ControlType -eq [System.Windows.Automation.ControlType]::Document) { $script:scopeDiagnostics.Boundary++; break }
        $script:scopeDiagnostics.Scopes++
        $scopeBounds = $scope.Current.BoundingRectangle
        $layoutScope = (Test-ComposerLayoutClass $scope.Current.ClassName) -and
            (Test-ComposerLayoutBounds $scopeBounds $composerBounds $window.Current.BoundingRectangle)
        if (-not $layoutScope -and -not (Test-ComposerScopeBounds $scopeBounds $composerBounds)) { $script:scopeDiagnostics.RejectedBounds++; continue }
        if ($layoutScope) { $script:scopeDiagnostics.Layout++ }
        $editIdentity = $edit.GetRuntimeId() -join '.'
        $otherComposers = @($scope.FindAll([System.Windows.Automation.TreeScope]::Descendants, $editCondition) | Where-Object {
            ($_.GetRuntimeId() -join '.') -ne $editIdentity -and -not $_.Current.IsOffscreen -and
            $_.Current.ClassName -match '(^|\s)ProseMirror(\s|$)'
        })
        if ($otherComposers.Count -gt 0) { $script:scopeDiagnostics.OtherEditors++; break }
        $controls = [System.Collections.Generic.List[object]]::new()
        foreach ($button in $scope.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)) {
            $script:scopeDiagnostics.Buttons++
            if ($button.Current.IsOffscreen) { $script:scopeDiagnostics.Offscreen++; continue }
            $kind = Get-SubmitControlKind $button.Current.Name
            if ($kind -eq 'Other') { $script:scopeDiagnostics.Unknown++; continue }
            if (-not (Test-ComposerControlBounds $button.Current.BoundingRectangle $composerBounds $scopeBounds -Layout:$layoutScope)) { $script:scopeDiagnostics.Outside++; continue }
            [void]$controls.Add([PSCustomObject]@{Button=$button; Kind=$kind; Depth=$depth})
        }
        if ($controls.Count -gt 0) {
            # Use the nearest composer frame exposing a submit/stop control.
            # Do not continue outward and pick another pane's identically named button.
            return $controls.ToArray()
        }
    }
    if (-not $Raw) {
        # Control View can omit presentational footer wrappers after mounting.
        # Raw View retains these wrappers, with the same geometry, uniqueness
        # and Document/window boundaries; never widen to a page-wide search.
        return (Get-ComposerControls $edit $window -Raw)
    }
}

function Get-NavigationAction($observation, $state, [long]$nowMs) {
    if ($observation.TargetCount -eq 1) { $state.TargetAcquired = $true }
    if ($observation.Prefilled) { $state.PrefillDispatched = $true }
    # Native UIA is addressed to a verified control and needs no foreground
    # focus. Navigation/prefill remain one-way transitions to prevent loops.
    if ($state.PrefillDispatched) { return 'Wait' }
    if (-not $observation.AppReady) { $state.AppKey = ''; return 'Wait' }
    if ($state.AppKey -ne $observation.AppKey) { $state.AppKey = $observation.AppKey; return 'Wait' }
    if ($observation.TargetCount -gt 1 -or $observation.Prefilled -or $observation.ConflictingDraft) { return 'Wait' }
    if (-not $state.TargetAcquired -and $observation.TargetCount -eq 0 -and $state.RouteAttempts -lt 6 -and ($nowMs - $state.LastRouteMs) -ge 8000) {
        $state.RouteAttempts++
        $state.LastRouteMs = $nowMs
        return 'Route'
    }
    if ($observation.TargetCount -eq 1 -and $state.PrefillAttempts -lt 3 -and ($nowMs - $state.LastPrefillMs) -ge 4000) {
        $state.PrefillAttempts++
        $state.PrefillDispatched = $true
        $state.LastPrefillMs = $nowMs
        return 'Prefill'
    }
    return 'Wait'
}

function Open-ConversationLink($link) {
    $opened = [CodexToolsContinueNative]::ShellExecute([IntPtr]::Zero, 'open', $link, $null, $null, 4)
    if ($opened.ToInt64() -le 32) { throw 'Unable to open the Codex conversation link.' }
}

function Update-ConversationNavigation {
    $action = Get-NavigationAction $script:navigationObservation $script:navigationState ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())
    if ($action -eq 'Route') {
        Write-Stage ('opening_target_conversation attempt=' + $script:navigationState.RouteAttempts)
        # Open the existing task first, without modifying an unseen draft.
        Open-ConversationLink $navigationUri
    } elseif ($action -eq 'Prefill') {
        Write-Stage ('prefilling_target_conversation attempt=' + $script:navigationState.PrefillAttempts)
        # Only the uniquely identified, empty target composer may be prefilled.
        Open-ConversationLink $uri
    }
}

function Get-ReadyCandidate([switch]$ObserveRunning) {
    $candidates = [System.Collections.Generic.List[object]]::new()
    $processes = @(Get-CodexDesktopProcesses)
    $counts = @{Windows=0; Titles=0; Editors=0; VisibleEditors=0; PromptMatches=0; MainMatches=0; SendControls=0; DisabledSend=0; RunningControls=0; ControlDepth=0}
    $lastAccessibilityError = $null
    $appWindows = @{}
    $targetDrafts = @{}
    $appForeground = $false
    $foregroundHandle = [CodexToolsContinueNative]::GetForegroundWindow()
    $scopeTrace = 'not_observed'
    foreach ($process in $processes) {
        if ($process.MainWindowHandle -eq 0) { continue }
        $counts.Windows++
        if ($process.MainWindowHandle -eq $foregroundHandle) { $appForeground = $true }
        try {
            $window = [System.Windows.Automation.AutomationElement]::FromHandle($process.MainWindowHandle)
            # Home may have no chat editor yet. A live web Document with an
            # enabled visible App button also proves the renderer is mounted;
            # native minimize/close buttons outside the Document do not qualify.
            $document = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $documentType)
            if ($null -ne $document -and -not $document.Current.IsOffscreen) {
                # Only readiness needs one button; do not enumerate every
                # transcript/tool button on each poll in long conversations.
                $appButton = $document.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $enabledButtonCondition)
                if ($null -ne $appButton) {
                    $appWindows[[string]$process.Id + ':' + [string]$process.MainWindowHandle] = $true
                }
            }
            $headings = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $titleCondition)
            $counts.Titles += $headings.Count
            $edits = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $editCondition)
            $counts.Editors += $edits.Count
            foreach ($edit in $edits) {
                if (-not $edit.Current.IsEnabled -or $edit.Current.IsOffscreen -or
                    -not $edit.Current.IsKeyboardFocusable) { continue }
                $counts.VisibleEditors++
                # Search fields and the outer web Document do not prove that
                # the App's chat renderer/composer has finished mounting.
                if ($edit.Current.ClassName -notmatch '(^|\s)ProseMirror(\s|$)') { continue }
                $appWindows[[string]$process.Id + ':' + [string]$process.MainWindowHandle] = $true
                $composerText = Get-ComposerText $edit
                if ($composerText -eq $prompt) { $counts.PromptMatches++ }
                $composerBounds = $edit.Current.BoundingRectangle
                if ($composerBounds.IsEmpty) { continue }
                $targetVisible = $false
                foreach ($heading in $headings) {
                    # A sidebar match alone is insufficient: the title must be in the main content above this composer.
                    if (Test-MainContentTitle $heading $edit $window) {
                        $targetVisible = $true
                        break
                    }
                }
                if (-not $targetVisible) { continue }
                $key = [string]$process.Id + ':' + ($edit.GetRuntimeId() -join '.')
                # Empty Chromium editors can expose their accessible placeholder
                # through text/value patterns. An actual different draft is protected.
                $placeholder = $composerText -eq $edit.Current.Name -and
                    $edit.Current.Name -match '^(Ask Codex|Work with Codex|\u548c Codex \u4e00\u8d77\u5de5\u4f5c)$'
                $targetDrafts[$key] = [PSCustomObject]@{
                    Prefilled = $composerText -eq $prompt
                    Conflicting = $composerText.Length -gt 0 -and $composerText -ne $prompt -and -not $placeholder
                }
                if ($composerText -ne $prompt -and -not $ObserveRunning) { continue }
                $counts.MainMatches++
                $controls = @(Get-ComposerControls $edit $window)
                $scopeTrace = (@($script:scopeDiagnostics.Keys | Sort-Object | ForEach-Object {$_ + ':' + $script:scopeDiagnostics[$_]}) -join ',')
                foreach ($control in $controls) {
                    $button = $control.Button
                    $kind = $control.Kind
                    $controlKey = $key + ':' + ($button.GetRuntimeId() -join '.')
                    $counts.ControlDepth = [Math]::Max($counts.ControlDepth, $control.Depth)
                    if ($kind -eq 'Running') {
                        if ($button.Current.IsEnabled) {
                            $counts.RunningControls++
                            if ($ObserveRunning -and (Test-StopControl $button.Current.Name)) {
                                [void]$candidates.Add([PSCustomObject]@{Key=$key; ControlKey=$controlKey; Kind='Stop'})
                            }
                        }
                        continue
                    }
                    if ($ObserveRunning) { continue }
                    if (-not $button.Current.IsEnabled) { $counts.DisabledSend++; continue }
                    $counts.SendControls++
                    $invoke = $null
                    if ($button.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$invoke)) {
                        [void]$candidates.Add([PSCustomObject]@{
                            Key = $key
                            ControlKey = $controlKey
                            Kind = 'Send'
                            Invoke = $invoke
                        })
                    }
                }
            }
        } catch {
            $lastAccessibilityError = $_.Exception.Message
        }
    }
    $script:navigationObservation = [PSCustomObject]@{
        AppReady = $appWindows.Count -gt 0
        AppForeground = $appForeground
        AppKey = (@($appWindows.Keys | Sort-Object) -join ',')
        TargetCount = $targetDrafts.Count
        Prefilled = @($targetDrafts.Values | Where-Object {$_.Prefilled}).Count -gt 0
        ConflictingDraft = @($targetDrafts.Values | Where-Object {$_.Conflicting}).Count -gt 0
    }
    $stage = if ($processes.Count -eq 0) { 'desktop_process_not_found' }
        elseif ($counts.Windows -eq 0) { 'waiting_for_desktop_window' }
        elseif ($appWindows.Count -eq 0) { 'waiting_for_app_shell' }
        elseif ($targetDrafts.Count -gt 1) { 'waiting_for_unique_target_composer' }
        elseif ($script:navigationObservation.ConflictingDraft) { 'target_has_different_draft' }
        elseif ($counts.Titles -eq 0) { 'waiting_for_target_title' }
        elseif ($counts.PromptMatches -eq 0) { 'waiting_for_prefilled_composer' }
        elseif ($counts.MainMatches -eq 0) { 'waiting_for_main_content_title' }
        elseif ($candidates.Count -eq 0 -and $counts.RunningControls -gt 0) { 'target_has_running_controls' }
        elseif ($candidates.Count -eq 0) { 'waiting_for_enabled_send_control' }
        else { 'composer_ready' }
    # Log counts, never draft/transcript text. Report the most advanced stage
    # once per poll, so background renderer processes cannot overwrite it.
    Write-Stage ($stage + ' windows=' + $counts.Windows + ' titles=' + $counts.Titles +
        ' editors=' + $counts.Editors + ' visible=' + $counts.VisibleEditors +
        ' prompt_matches=' + $counts.PromptMatches + ' main_matches=' + $counts.MainMatches +
        ' sends=' + $counts.SendControls + ' disabled_sends=' + $counts.DisabledSend +
        ' running_controls=' + $counts.RunningControls + ' control_depth=' + $counts.ControlDepth +
        ' target_composers=' + $targetDrafts.Count + ' foreground=' + $appForeground + ' scope=' + $scopeTrace)
    if ($lastAccessibilityError) { Write-Stage ('accessibility_error: ' + $lastAccessibilityError) }
    if ($targetDrafts.Count -gt 1 -or $script:navigationObservation.ConflictingDraft) { return $null }
    # Never choose the first same-title draft while the deep-link navigation is still loading.
    return (Select-UniqueReadyCandidate -candidates $candidates.ToArray())
}

function Test-RunningTransition($submitted, $observed) {
    return $null -ne $submitted -and $null -ne $observed -and
        $submitted.Kind -eq 'Send' -and $observed.Kind -eq 'Stop' -and
        $submitted.Key -eq $observed.Key
}

function Wait-RunningTransition($submitted) {
    $runningDeadline = [DateTime]::UtcNow.AddSeconds(4)
    $observedKey = $null
    do {
        # Read only: no navigation, prefill, second Invoke or Stop invocation.
        $observed = Get-ReadyCandidate -ObserveRunning
        if (Test-RunningTransition $submitted $observed) {
            if ($observedKey -eq $observed.Key) { return $true }
            $observedKey = $observed.Key
        } else { $observedKey = $null }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $runningDeadline)
    return $false
}

$deadline = [DateTime]::UtcNow.AddSeconds(110)
$previousKey = $null
while ([DateTime]::UtcNow -lt $deadline) {
    $candidate = Get-ReadyCandidate
    if ($null -eq $candidate) {
        $previousKey = $null
        Update-ConversationNavigation
    } elseif ($previousKey -eq ($candidate.Key + ':' + $candidate.ControlKey)) {
        # The parent refreshes the task state after navigation, immediately before authorizing a single submit.
        [Console]::Out.WriteLine('READY')
        [Console]::Out.Flush()
        if ([Console]::In.ReadLine() -ne 'SUBMIT') { exit 0 }
        # Re-query after the handshake: navigation, edits, disabled buttons or running turns revoke readiness.
        $verified = Get-ReadyCandidate
        if ($null -eq $verified -or $verified.Key -ne $candidate.Key -or $verified.ControlKey -ne $candidate.ControlKey) {
            # The goal bar or tab can mount between READY and SUBMIT. No
            # Invoke happened, so revoke authorization and wait for fresh
            # readiness; the parent must recheck the original task each time.
            [Console]::Out.WriteLine('REVOKED')
            [Console]::Out.Flush()
            Write-Stage 'readiness_revoked_before_invoke'
            $previousKey = $null
            Start-Sleep -Milliseconds 400
            continue
        }
        # Never retry Invoke: even an exception can mean the App already accepted the message.
        $script:invokeAttempted = $true
        [Console]::Out.WriteLine('INVOKING')
        [Console]::Out.Flush()
        $verified.Invoke.Invoke()
        [Console]::Out.WriteLine('SUBMITTED')
        [Console]::Out.Flush()
        if (Wait-RunningTransition $verified) {
            Write-Stage 'confirmed_send_to_stop_transition'
            [Console]::Out.WriteLine('RUNNING')
            [Console]::Out.Flush()
        }
        exit 0
    } else {
        $previousKey = $candidate.Key + ':' + $candidate.ControlKey
    }
    $pollDelay = 900
    if ($null -ne $candidate) { $pollDelay = 300 }
    Start-Sleep -Milliseconds $pollDelay
}
throw ('Timed out waiting for Codex readiness: ' + $script:lastReason)
