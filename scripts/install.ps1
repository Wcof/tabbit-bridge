#requires -version 5.1
<#
tabbit-bridge 一键安装脚本（Windows）
用法: iwr -useb https://<release>/install.ps1 | iex
#>
$ErrorActionPreference = 'Stop'

$Repo = if ($env:REPO) { $env:REPO } else { 'Wcof/tabbit-bridge' }

# 解析最新稳定版（GitHub API）
function Resolve-StableVersion {
    if ($env:VERSION -and $env:VERSION -ne 'latest') { return $env:VERSION }
    $api = "https://api.github.com/repos/$Repo/releases/latest"
    $Headers = @{ Accept='application/vnd.github+json'; 'User-Agent'='tabbit-bridge-installer' }
    (Invoke-RestMethod -UseBasicParsing -Uri $api -Headers $Headers).tag_name
}
$Version = Resolve-StableVersion
Write-Host "[install] 稳定版本: $Version" -ForegroundColor Green

$InstallDir = if ($env:PREFIX) { $env:PREFIX } else { Join-Path $env:LOCALAPPDATA 'tabbit-bridge' }

# 1. 检测平台
$Arch = if ([Environment]::Is64BitOperatingSystem) { 'x86_64' } else { 'x86' }
$Target = "$Arch-pc-windows-msvc"
Write-Host "[install] 目标平台: $Target" -ForegroundColor Green

# 2. 下载
$Url = "https://github.com/$Repo/releases/download/$Version/tabbit-bridge-$Target.zip"
Write-Host "[install] 下载: $Url" -ForegroundColor Green
$Tmp = Join-Path $env:TEMP 'tabbit-bridge-install'
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
$Zip = Join-Path $Tmp 'bridge.zip'
try {
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Zip
} catch {
    Write-Host "[install] 下载失败: $_" -ForegroundColor Red
    exit 1
}

# 3. 解压安装
Expand-Archive -Path $Zip -DestinationPath $Tmp -Force
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$ExePath = Join-Path $InstallDir 'tabbit-bridge.exe'
Copy-Item (Join-Path $Tmp 'tabbit-bridge.exe') $ExePath -Force
Write-Host "[install] 二进制已安装至: $ExePath" -ForegroundColor Green

# 4. 加入 PATH（用户级）
$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable('Path', "$UserPath;$InstallDir", 'User')
    Write-Host "[install] 已加入用户 PATH" -ForegroundColor Green
}

# 5. 首次自举配置并取回 token
$Token = & $ExePath --print-token
if ($LASTEXITCODE -ne 0) {
    Write-Host "[install] 配置自举失败" -ForegroundColor Red
    exit 1
}

# 6. 注册 Windows 服务（需管理员）
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($isAdmin) {
    Write-Host "[install] 注册 Windows 服务..." -ForegroundColor Green
    & $ExePath --install
    if ($LASTEXITCODE -ne 0) { Write-Host "[install] 服务注册失败，退化为登录时运行" -ForegroundColor Yellow }
} else {
    Write-Host "[install] 非管理员，跳过服务注册。可手动以管理员身份运行 --install" -ForegroundColor Yellow
}

# 7. 打印 token
$CfgPath = Join-Path $env:APPDATA 'tabbit-bridge\config.toml'
Write-Host ""
Write-Host "================ tabbit-bridge 安装完成 ================" -ForegroundColor Yellow
Write-Host "监听地址: 127.0.0.1（端口见 config.toml）"
Write-Host "配置路径: $CfgPath"
Write-Host "TOKEN（填入妙招脚本，请勿泄露）:" -ForegroundColor Cyan
Write-Host "$Token" -ForegroundColor Cyan
Write-Host "========================================================" -ForegroundColor Yellow
Write-Host "如需卸载: $ExePath --uninstall" -ForegroundColor Green
