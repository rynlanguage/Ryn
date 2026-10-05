[CmdletBinding()]
param(
    [string] $InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\Ryn')
)

$ErrorActionPreference = 'Stop'
$InstallDir = [IO.Path]::GetFullPath($InstallDir)
$repository = 'rynlanguage/Ryn'
$apiBase = "https://api.github.com/repos/$repository"

function Get-GitHubToken {
    if ($env:GH_TOKEN) { return $env:GH_TOKEN }
    if ($env:GITHUB_TOKEN) { return $env:GITHUB_TOKEN }

    if (Get-Command gh -ErrorAction SilentlyContinue) {
        try {
            $token = (& gh auth token --hostname github.com 2>$null | Select-Object -First 1)
            if ($token) { return $token.Trim() }
        }
        catch {
            # Public releases do not require a GitHub token.
        }
    }
}

function Get-GitHubHeaders {
    $headers = @{
        Accept = 'application/vnd.github+json'
        'X-GitHub-Api-Version' = '2022-11-28'
    }
    $token = Get-GitHubToken
    if ($token) { $headers.Authorization = "Bearer $token" }
    return $headers
}

function Save-GitHubAsset {
    param(
        [Parameter(Mandatory)] $Asset,
        [Parameter(Mandatory)] [string] $Destination,
        [Parameter(Mandatory)] [hashtable] $Headers
    )

    $downloadHeaders = @{}
    foreach ($key in $Headers.Keys) { $downloadHeaders[$key] = $Headers[$key] }
    $downloadHeaders.Accept = 'application/octet-stream'
    Invoke-WebRequest -Uri $Asset.url -Headers $downloadHeaders -OutFile $Destination
}

$headers = Get-GitHubHeaders
$workDir = Join-Path ([IO.Path]::GetTempPath()) "ryn-install-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $workDir | Out-Null

try {
    Write-Host 'Looking up the latest Ryn release...'
    $release = Invoke-RestMethod -Uri "$apiBase/releases/latest" -Headers $headers
    if ($release.tag_name -notmatch '^v(\d+\.\d+\.\d+)$') {
        throw "The latest release tag '$($release.tag_name)' is not a stable Ryn version."
    }
    $version = $Matches[1]
    $archiveName = "ryn-$version-windows-x86_64.zip"
    $archiveAsset = $release.assets | Where-Object name -EQ $archiveName | Select-Object -First 1
    $checksumAsset = $release.assets | Where-Object name -EQ "$archiveName.sha256" | Select-Object -First 1
    if (-not $archiveAsset -or -not $checksumAsset) {
        throw "Release $($release.tag_name) does not contain the expected Windows x86_64 archive and checksum."
    }

    $archivePath = Join-Path $workDir $archiveName
    $checksumPath = "$archivePath.sha256"
    Save-GitHubAsset -Asset $archiveAsset -Destination $archivePath -Headers $headers
    Save-GitHubAsset -Asset $checksumAsset -Destination $checksumPath -Headers $headers

    $checksumText = Get-Content -LiteralPath $checksumPath -Raw
    $expectedHash = [regex]::Match($checksumText, '^\s*([0-9a-fA-F]{64})').Groups[1].Value
    if (-not $expectedHash) { throw 'The release checksum file is malformed.' }
    $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
    if ($actualHash -ne $expectedHash) { throw 'The downloaded archive failed its SHA-256 check.' }

    $extractDir = Join-Path $workDir 'extracted'
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir
    $executable = Get-ChildItem -LiteralPath $extractDir -Filter 'ryn.exe' -File -Recurse | Select-Object -First 1
    if (-not $executable) { throw 'The release archive does not contain ryn.exe.' }

    $null = New-Item -ItemType Directory -Path $InstallDir -Force
    $destination = Join-Path $InstallDir 'ryn.exe'
    Copy-Item -LiteralPath $executable.FullName -Destination $destination -Force

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    $pathComparison = [StringComparer]::OrdinalIgnoreCase
    if (-not ($pathEntries | Where-Object { $pathComparison.Equals($_.TrimEnd('\'), $InstallDir.TrimEnd('\')) })) {
        $updatedPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable('Path', $updatedPath, 'User')
    }
    if (-not ($env:Path -split ';' | Where-Object { $pathComparison.Equals($_.TrimEnd('\'), $InstallDir.TrimEnd('\')) })) {
        $env:Path = "$InstallDir;$env:Path"
    }

    Write-Host "Installed Ryn $version to $destination"
    Write-Host 'Open a new terminal to use ryn from PATH, or run: ryn --version'
}
finally {
    Remove-Item -LiteralPath $workDir -Recurse -Force -ErrorAction SilentlyContinue
}
