# PowerShell build wrapper to set C11 for aws-lc-sys
$env:AWS_LC_SYS_C_STD = "c11"
$env:CFLAGS = "/std:c11"
$env:CMAKE_C_STANDARD = "11"
$env:CMAKE_C_FLAGS = "/std:c11"
$env:AWS_LC_SYS_CMAKE_BUILDER = "0"

Write-Host "Building with C11 flags set..." -ForegroundColor Green
Write-Host "AWS_LC_SYS_C_STD = $env:AWS_LC_SYS_C_STD" -ForegroundColor Yellow
Write-Host "CFLAGS = $env:CFLAGS" -ForegroundColor Yellow

cargo $args
