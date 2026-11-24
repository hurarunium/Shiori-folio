# 余りのPC(Windowsのサーバー)に、OpenSSHサーバーを入れて、開発機から鍵認証でつなげるようにする。
# 管理者権限のPowerShellで実行する。
#   powershell -ExecutionPolicy Bypass -File .\setup-openssh.ps1
# 受け付けるのは、Tailscaleのアドレス(100.64.0.0/10)からだけ。パスワードでのログインは、鍵で通ったあとに止める(下の手順)。
$ErrorActionPreference = 'Stop'

# 開発機の公開鍵(秘密鍵は開発機から出ない)
$pubKey = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINj/lmIbV6R5FmWe1eXlfPLCf4NJ8V32yT/H4E/f1QOp dev-to-shiori-server'

Write-Host '1. OpenSSHサーバーを入れる' -ForegroundColor Cyan
$cap = Get-WindowsCapability -Online -Name 'OpenSSH.Server*' | Select-Object -First 1
if ($cap.State -ne 'Installed') { Add-WindowsCapability -Online -Name $cap.Name | Out-Null }

Write-Host '2. 管理者アカウント用の鍵を置く(administrators_authorized_keys)' -ForegroundColor Cyan
$dir = Join-Path $env:ProgramData 'ssh'
New-Item -ItemType Directory -Force $dir | Out-Null
$file = Join-Path $dir 'administrators_authorized_keys'
if (-not (Test-Path $file) -or -not (Select-String -Path $file -SimpleMatch $pubKey -Quiet)) {
    Add-Content -Path $file -Value $pubKey -Encoding ascii
}
# 所有者はAdministratorsとSYSTEMだけ(OpenSSHの要求)。SIDで指定する(日本語名のWindowsでも通る)
icacls $file /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F' | Out-Null

Write-Host '3. 起動の設定(PowerShellを既定のシェルに。自動起動)' -ForegroundColor Cyan
New-Item -Path 'HKLM:\SOFTWARE\OpenSSH' -Force | Out-Null
New-ItemProperty -Path 'HKLM:\SOFTWARE\OpenSSH' -Name DefaultShell -Value 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' -PropertyType String -Force | Out-Null
Set-Service -Name sshd -StartupType Automatic
Start-Service sshd

Write-Host '4. ファイアウォール: Tailscaleのアドレス(100.64.0.0/10)からだけ受け付ける' -ForegroundColor Cyan
Get-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -ErrorAction SilentlyContinue | Disable-NetFirewallRule
Remove-NetFirewallRule -Name 'ShioriOpenSSH-Tailscale' -ErrorAction SilentlyContinue
New-NetFirewallRule -Name 'ShioriOpenSSH-Tailscale' -DisplayName 'OpenSSH (Tailscaleのみ)' -Direction Inbound -Protocol TCP -LocalPort 22 -RemoteAddress '100.64.0.0/10' -Action Allow -Profile Any | Out-Null

Write-Host ''
Write-Host '--- 結果 ---' -ForegroundColor Green
Get-Service sshd | Select-Object Name, Status, StartType | Format-Table -AutoSize
Write-Host ('ログイン名(開発機の ssh で使う。円記号の後ろだけ): ' + (whoami))
Write-Host ('Tailscaleのアドレス: ' + ((tailscale ip -4) -join ' '))
Write-Host 'この2行を、Claudeに伝えてください。'
