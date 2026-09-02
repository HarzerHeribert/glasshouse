<#
.SYNOPSIS
    Run the Windows CI batch under the CI user's INTERACTIVE logon.

.DESCRIPTION
    WHY THIS FILE EXISTS
    --------------------
    `glasshouse-windows-ci` drives the VM over ssh with a public key. Windows
    OpenSSH builds that session's token with an S4U logon, which has no
    primary credentials, and Windows Credential Manager is scoped to a logon
    session rather than to a user. So `CredReadW`, `CredWriteW` and
    `CredDeleteW` all answer 1312, ERROR_NO_SUCH_LOGON_SESSION, in that
    session -- measured directly on the VM on 2026-09-02, with no Rust and no
    `keyring` in the call:

        whoami            : glasshouse-ci\glasshouse
        process SessionId : 0
        CredReadW(probe)  : FAILED err=1312
        CredWriteW(test)  : FAILED err=1312

    That is why five `secret_native` round-trip tests printed
    "SKIPPED: the native secure store would not open in this session" on the
    2026-09-02 gate: Glasshouse was reaching a real Credential Manager and
    being refused by the session, exactly as its fallback is designed to do.

    The same probe, run by a scheduled task registered with
    `-LogonType Interactive` under the same user, gets the console session's
    full token:

        process SessionId : 1
        CredReadW(probe)  : FAILED err=1168   (ERROR_NOT_FOUND -> keyring NoEntry -> the store answered)
        CredWriteW(test)  : OK
        CredReadW(test)   : OK (round trip)
        CredDeleteW(test) : OK

    So this script runs `run-glasshouse-ci.cmd` through such a task and
    streams its log back over the ssh channel. No password and no stored
    credential is involved: `-LogonType Interactive` reuses the logon the
    console session already has, which is why it REQUIRES a logged-on user
    and fails loudly below when there is none.

    WHAT IT DELIBERATELY DOES NOT DO
    --------------------------------
    It never kills anything (practice section 72). If the ssh channel dies,
    the task keeps running on the VM -- the same far-side-outlives-near-side
    asymmetry the driver's `tasklist` idle check already exists to catch.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('all', 'stable', 'build', 'test', 'msrv')]
    [string]$Mode,

    [string]$TaskName = 'GlasshouseCI',

    # Generous rather than tight: a run this kills mid-test looks exactly
    # like a product hang in the log it leaves behind.
    [int]$TimeoutMinutes = 240
)

$ErrorActionPreference = 'Stop'

$ciRoot = 'C:\ci'
$runner = Join-Path $ciRoot 'run-glasshouse-ci.cmd'
$log = Join-Path $ciRoot "ci-$Mode.log"
$wrapper = Join-Path $ciRoot "ci-run-$Mode.cmd"

if (-not (Test-Path -LiteralPath $runner)) {
    throw "Runner not found: $runner"
}

# 1. There must BE an interactive logon to borrow. Without one the task is
#    registered happily and then never runs, which is the failure mode this
#    check exists to turn into a sentence.
$sessions = @(query.exe session 2>$null)
$interactive = $sessions | Where-Object {
    $_ -match '^\s*[>\s]?\S+\s+' + [regex]::Escape($env:USERNAME) + '\s+\d+\s+Active'
}
if (-not $interactive) {
    Write-Output ($sessions -join "`n")
    throw ("No Active interactive session for '$($env:USERNAME)' on this VM. " +
        'Windows Credential Manager is scoped to a logon session, so the CI batch ' +
        'would run in the ssh session (session 0) and every credential call would ' +
        'answer ERROR_NO_SUCH_LOGON_SESSION. Log the CI user in at the console ' +
        '(autologon) and retry.')
}

# 2. Refuse to start on top of a run that is still going, rather than
#    replacing its task definition underneath it.
$existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($existing -and $existing.State -eq 'Running') {
    throw "Scheduled task '$TaskName' is already running on this VM; not touching it."
}

# 3. A wrapper, so the redirection is cmd's own rather than something that
#    has to survive being quoted into a task argument.
@"
@echo off
call "$runner" $Mode > "$log" 2>&1
exit /b %errorlevel%
"@ | Set-Content -LiteralPath $wrapper -Encoding ASCII

if (Test-Path -LiteralPath $log) { Remove-Item -LiteralPath $log -Force }

$action = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument "/d /c `"$wrapper`""
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -MultipleInstances IgnoreNew `
    -ExecutionTimeLimit ([TimeSpan]::FromMinutes($TimeoutMinutes))

Register-ScheduledTask -TaskName $TaskName -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Write-Output "Running C:\ci\run-glasshouse-ci.cmd $Mode under an interactive logon (scheduled task '$TaskName')."

$startedAt = (Get-Date).AddSeconds(-5)
Start-ScheduledTask -TaskName $TaskName

# 4. Follow the log while it runs. `FileShare.ReadWrite` because cmd still
#    has it open; a plain Get-Content would fail or block.
$position = 0L
function Write-NewLogBytes {
    if (-not (Test-Path -LiteralPath $log)) { return }
    $chunk = ''
    $stream = [System.IO.File]::Open($log, 'Open', 'Read', 'ReadWrite')
    try {
        if ($stream.Length -lt $script:position) { $script:position = 0L }
        [void]$stream.Seek($script:position, 'Begin')
        $reader = New-Object System.IO.StreamReader($stream)
        $chunk = $reader.ReadToEnd()
        $script:position = $stream.Position
    } finally {
        $stream.Dispose()
    }
    if ($chunk) { [Console]::Out.Write($chunk) }
}

# Two deadlines, because "never started" and "never finished" are different
# failures and only one of them is about the batch.
$startDeadline = (Get-Date).AddMinutes(3)
$deadline = (Get-Date).AddMinutes($TimeoutMinutes + 5)
$running = $false
while ($true) {
    Start-Sleep -Seconds 3
    Write-NewLogBytes

    $task = Get-ScheduledTask -TaskName $TaskName
    $info = $task | Get-ScheduledTaskInfo
    if ($task.State -eq 'Running') {
        $running = $true
    } elseif ($running -or ($info.LastRunTime -and $info.LastRunTime -ge $startedAt)) {
        # Either we watched it run, or it began and ended between two polls.
        break
    } elseif ((Get-Date) -gt $startDeadline) {
        throw ("Scheduled task '$TaskName' did not start within three minutes " +
            "(state '$($task.State)', last result $($info.LastTaskResult)). " +
            'An Interactive-logon task runs only while its user is logged on at the console.')
    }

    if ((Get-Date) -gt $deadline) {
        throw ("Scheduled task '$TaskName' has not finished after $TimeoutMinutes minutes. " +
            'It is still running on the VM and was NOT killed (practice section 72); ' +
            'check `tasklist` there before starting another run.')
    }
}
Write-NewLogBytes

$result = (Get-ScheduledTask -TaskName $TaskName | Get-ScheduledTaskInfo).LastTaskResult
Write-Output "Scheduled task '$TaskName' finished with exit code $result."
if ($result -eq 267011) {
    throw 'The task never ran: no interactive logon was available when it started.'
}
exit $result
