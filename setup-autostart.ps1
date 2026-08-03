#Requires -Version 5.1
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallDir,

    [int]$DelaySeconds = 15,

    [string]$TaskName = 'ClipboardShare'
)

$ErrorActionPreference = 'Stop'
$exe = Join-Path $InstallDir 'clipboard_share.exe'
if (-not (Test-Path -LiteralPath $exe)) {
    throw "找不到可执行文件: $exe"
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$userId = $identity.Name
if ([string]::IsNullOrWhiteSpace($userId)) {
    throw '无法解析当前用户'
}

# 避免与注册表 Run 键重复启动
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (Get-ItemProperty -Path $runKey -Name 'ClipboardShare' -ErrorAction SilentlyContinue) {
    Remove-ItemProperty -Path $runKey -Name 'ClipboardShare' -Force
    Write-Host '已移除 HKCU Run\ClipboardShare'
}

Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Unregister-ScheduledTask -TaskName 'ClipboardShareStart' -Confirm:$false -ErrorAction SilentlyContinue

$action = New-ScheduledTaskAction -Execute $exe -Argument 'start' -WorkingDirectory $InstallDir
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $userId
$trigger.Delay = "PT${DelaySeconds}S"
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -ExecutionTimeLimit ([TimeSpan]::Zero)
$principal = New-ScheduledTaskPrincipal -UserId $userId -LogonType Interactive -RunLevel Limited

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $action `
    -Trigger $trigger `
    -Settings $settings `
    -Principal $principal `
    -Description "ClipboardShare 登录后延时 ${DelaySeconds}s 自启" `
    -Force | Out-Null

$task = Get-ScheduledTask -TaskName $TaskName
Write-Host "TaskName=$($task.TaskName)"
Write-Host "State=$($task.State)"
Write-Host "Delay=$($task.Triggers[0].Delay)"
Write-Host "User=$($task.Principal.UserId)"
Write-Host "LogonType=$($task.Principal.LogonType)"
Write-Host "Action=$($task.Actions[0].Execute) $($task.Actions[0].Arguments)"
Write-Host "WorkDir=$($task.Actions[0].WorkingDirectory)"
