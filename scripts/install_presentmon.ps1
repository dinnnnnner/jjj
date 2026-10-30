$ErrorActionPreference = 'Stop'
$destination = Join-Path $PSScriptRoot '..\tools\presentmon'
$null = New-Item -ItemType Directory -Force -Path $destination
$executable = Join-Path $destination 'PresentMon-2.3.1-x64.exe'
$expectedHash = '364e5d98d4d134bd54dd25c22ed2ca2f4883f8bc3ed6502bee0c151e3436d30c'
if (!(Test-Path -LiteralPath $executable) -or
    (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash -ne $expectedHash) {
    Invoke-WebRequest -Uri 'https://github.com/GameTechDev/PresentMon/releases/download/v2.3.1/PresentMon-2.3.1-x64.exe' -OutFile $executable
}
if ((Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash -ne $expectedHash) {
    throw 'PresentMon SHA256 verification failed; do not run the downloaded file.'
}
Invoke-WebRequest -Uri 'https://raw.githubusercontent.com/GameTechDev/PresentMon/v2.3.1/LICENSE.txt' -OutFile (Join-Path $destination 'LICENSE.txt')
Write-Host "PresentMon ready: $executable"
