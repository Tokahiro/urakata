# Downloads Apollo 0.4.6, checks its hash and copies the four SudoVDA
# package files into -OutDir. Every file is checked against the hashes that
# urakata pins (crates/urakata/src/driver.rs), so the installer never
# bundles another build. Needs 7-Zip (7z.exe) on PATH, as on GitHub's
# Windows runners.
param(
    [Parameter(Mandatory)] [string] $OutDir,
    [string] $Cache = "$env:RUNNER_TEMP\apollo"
)
$ErrorActionPreference = 'Stop'

$url = 'https://github.com/ClassicOldSong/Apollo/releases/download/v0.4.6/Apollo-0.4.6.exe'
$apolloSha = '42B2AEFAACB3474511517A56B96EE9F0517F30AC38B5DD2FDA9FD5B478F5021A'
$files = [ordered]@{
    'SudoVDA.inf' = 'AD69AC682756F0CF339B081FAC7E6E8159FDF2CA01CA69DF8945C7246C286925'
    'sudovda.cat' = '2F9189DE5604BEC9D86F51640CC540639E394D9AD0F8E689129375E95F2D22F8'
    'SudoVDA.dll' = '47EE263CB5DE9382C6630A2D7F3DAFEC4A49419F953BEEC869CA5DD0C460FF63'
    # Checked by its thumbprint below; urakata checks it again at install.
    'sudovda.cer' = $null
}
$thumbprint = '3C918FC73525AD8B1521B6DB26B71F694277CC49'

New-Item -ItemType Directory -Force -Path $Cache, $OutDir | Out-Null
$exe = Join-Path $Cache 'Apollo-0.4.6.exe'
if (-not (Test-Path $exe) -or (Get-FileHash $exe -Algorithm SHA256).Hash -ne $apolloSha) {
    Invoke-WebRequest -Uri $url -OutFile $exe -UseBasicParsing
}
$got = (Get-FileHash $exe -Algorithm SHA256).Hash
if ($got -ne $apolloSha) { throw "Apollo-0.4.6.exe has SHA-256 $got, expected $apolloSha" }

$extract = Join-Path $Cache 'extract'
if (Test-Path $extract) { Remove-Item -Recurse -Force $extract }
& 7z x $exe "-o$extract" 'drivers\sudovda\*' -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw "7z failed with exit code $LASTEXITCODE" }
$src = Join-Path $extract 'drivers\sudovda'

foreach ($name in $files.Keys) {
    $path = Join-Path $src $name
    if (-not (Test-Path $path)) { throw "$name is missing in the Apollo package" }
    $expected = $files[$name]
    if ($expected) {
        $hash = (Get-FileHash $path -Algorithm SHA256).Hash
        if ($hash -ne $expected) { throw "$name has SHA-256 $hash, expected $expected" }
    }
    Copy-Item $path (Join-Path $OutDir $name) -Force
}
$cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new((Join-Path $OutDir 'sudovda.cer'))
if ($cert.Thumbprint -ne $thumbprint) { throw "sudovda.cer has thumbprint $($cert.Thumbprint), expected $thumbprint" }
Write-Host "SudoVDA package ready in $OutDir"
