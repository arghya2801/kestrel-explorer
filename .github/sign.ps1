# Signs each file given with the release certificate: SIGNING_PFX (base64 .pfx) and SIGNING_PASSWORD.
param([Parameter(Mandatory)] [string[]]$Files)
$pfx = Join-Path $env:RUNNER_TEMP signing.pfx
[IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:SIGNING_PFX))
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName | Select-Object -Last 1
& $signtool.FullName sign /f $pfx /p $env:SIGNING_PASSWORD /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 /d Kestrel $Files
$failed = $LASTEXITCODE
Remove-Item $pfx
if ($failed) { throw "signtool failed" }
