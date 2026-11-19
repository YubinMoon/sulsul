# 술술 Windows 설치 파일(x64)을 만듭니다.
#
#   1. 릴리스 빌드: sulsul-app(기본 기능 = Vulkan)과 글 다듬기 실행기 sulsul-llm-runner
#   2. 넣을 파일을 target\installer\stage에 모음
#      - 실행 파일, DirectML.dll, VC++ 런타임 DLL
#      - Vulkan 로더(vulkan-1.dll): LunarG의 Vulkan 런타임 묶음을 내려받아 꺼냄(버전·SHA-256 고정)
#      - 라이선스: 술술, Rust 크레이트 고지(cargo-about로 만듦), installer\licenses의 네이티브 라이브러리 고지
#   3. Inno Setup으로 컴파일: target\installer\sulsul-<버전>-windows-x64-setup.exe
#
# 필요한 것: AGENTS.md의 빌드 도구(Rust, CMake, LLVM, Vulkan SDK), Visual Studio C++ 도구,
#            Inno Setup 6 (winget install --id JRSoftware.InnoSetup -e),
#            cargo-about (cargo install cargo-about --locked --features cli), PowerShell 7
#
# 사용: pwsh -File installer\build.ps1 [-SkipBuild]
#Requires -Version 7
param(
    # 이미 빌드한 target\release를 그대로 씁니다.
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$root = Split-Path -Parent $PSScriptRoot
$release = Join-Path $root 'target\release'
$outDir = Join-Path $root 'target\installer'
$stage = Join-Path $outDir 'stage'
$cache = Join-Path $outDir 'cache'

# Vulkan 로더. 드라이버가 없는 컴퓨터에서도 앱이 뜨도록 설치 폴더에 넣습니다(드라이버가 없으면 CPU로 인식).
# 버전을 올릴 때는 아래 세 값을 함께 바꿉니다.
$vulkanVersion = '1.4.363.0'
$vulkanUrl = "https://sdk.lunarg.com/sdk/download/$vulkanVersion/windows/VulkanRT-X64-$vulkanVersion-Components.zip"
$vulkanSha256 = 'a25a927aa8b9f0371048f1861cf88ac3b9bc9b1fb332c42d897c8ab32695769a'

# 버전은 워크스페이스 Cargo.toml의 [workspace.package] version을 씁니다.
$versionLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "([^"]+)"' | Select-Object -First 1
if (-not $versionLine) { throw 'Cargo.toml에서 version을 찾지 못했습니다.' }
$version = $versionLine.Matches[0].Groups[1].Value

$iscc = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6(ISCC.exe)이 없습니다. winget install --id JRSoftware.InnoSetup -e 로 설치하세요.' }
if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
    throw 'cargo-about이 없습니다. cargo install cargo-about --locked --features cli 로 설치하세요.'
}

if (-not $SkipBuild) {
    if (-not $env:VULKAN_SDK) { throw 'VULKAN_SDK 환경 변수가 없습니다. Vulkan SDK를 설치하고 터미널을 다시 여세요.' }
    Push-Location $root
    try {
        cargo build --release -p sulsul-app -p sulsul-llm-runner
        if ($LASTEXITCODE) { throw "cargo build가 실패했습니다 (종료 코드 $LASTEXITCODE)." }
    } finally {
        Pop-Location
    }
}

# 실행 파일이 불러오는 VC++ 런타임 DLL. Windows에 기본으로 없을 수 있어 설치 폴더에 함께 넣습니다
# (Visual Studio 재배포 폴더의 파일은 앱과 함께 배포해도 됩니다).
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual Studio C++ 도구를 찾지 못했습니다.' }
$crt = Get-ChildItem (Join-Path $vs 'VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT') -Directory |
    Sort-Object FullName | Select-Object -Last 1
if (-not $crt) { throw 'VC++ 재배포 폴더(Microsoft.VC*.CRT)를 찾지 못했습니다.' }
$runtime = 'msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll'

Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
$licenses = Join-Path $stage 'licenses'
New-Item -ItemType Directory -Force $licenses, $cache | Out-Null

foreach ($name in 'sulsul.exe', 'sulsul-llm-runner.exe', 'DirectML.dll') {
    $source = Join-Path $release $name
    if (-not (Test-Path $source)) { throw "$source 이 없습니다. -SkipBuild 없이 다시 실행하세요." }
    # DirectML.dll은 ort가 만든 심볼릭 링크일 수 있어 내용을 읽어 복사합니다.
    [System.IO.File]::WriteAllBytes((Join-Path $stage $name), [System.IO.File]::ReadAllBytes($source))
}
foreach ($name in $runtime) {
    Copy-Item (Join-Path $crt.FullName $name) $stage
}

# Vulkan 런타임 묶음: 한 번 내려받아 target\installer\cache에 두고, 해시가 맞을 때만 씁니다.
$vulkanZip = Join-Path $cache "VulkanRT-X64-$vulkanVersion-Components.zip"
if (-not (Test-Path $vulkanZip)) {
    Write-Host "Vulkan 런타임 $vulkanVersion 내려받는 중..."
    Invoke-WebRequest -Uri $vulkanUrl -OutFile "$vulkanZip.part"
    Move-Item "$vulkanZip.part" $vulkanZip -Force
}
$hash = (Get-FileHash $vulkanZip -Algorithm SHA256).Hash.ToLowerInvariant()
if ($hash -ne $vulkanSha256) {
    Remove-Item $vulkanZip -Force
    throw "Vulkan 런타임 묶음의 SHA-256이 다릅니다 ($hash). 파일을 지웠으니 다시 실행하거나 고정한 값을 확인하세요."
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($vulkanZip)
try {
    $prefix = "VulkanRT-X64-$vulkanVersion-Components/"
    foreach ($pair in @(
            @("${prefix}x64/vulkan-1.dll", (Join-Path $stage 'vulkan-1.dll')),
            @("${prefix}VulkanRT-License.txt", (Join-Path $licenses 'VulkanRT-LICENSE.txt')))) {
        $entry = $zip.GetEntry($pair[0])
        if (-not $entry) { throw "Vulkan 런타임 묶음에 $($pair[0]) 이 없습니다." }
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $pair[1], $true)
    }
} finally {
    $zip.Dispose()
}

# 라이선스
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $licenses 'sulsul-LICENSE.txt')
Copy-Item (Join-Path $root 'crates\sulsul-app\assets\fonts\Pretendard-LICENSE.txt') $licenses
Copy-Item (Join-Path $root 'crates\sulsul-vad\model\silero_vad-LICENSE.txt') $licenses
Copy-Item (Join-Path $PSScriptRoot 'licenses\*.txt') $licenses
Push-Location $root
try {
    # 워크스페이스 전체(개발용 sulsul-cli 포함)의 Rust 의존성을 모읍니다. 설치 파일에 든 것보다 조금 많을 수 있습니다.
    cargo about generate --workspace --locked --fail -c installer\about.toml installer\about.hbs `
        -o (Join-Path $licenses 'THIRD-PARTY-NOTICES.txt')
    if ($LASTEXITCODE) { throw "cargo about이 실패했습니다 (종료 코드 $LASTEXITCODE). installer\about.toml의 accepted를 확인하세요." }
} finally {
    Pop-Location
}

& $iscc /Qp "/DAppVersion=$version" "/DStageDir=$stage" "/O$outDir" (Join-Path $PSScriptRoot 'sulsul.iss')
if ($LASTEXITCODE) { throw "Inno Setup 컴파일이 실패했습니다 (종료 코드 $LASTEXITCODE)." }

$setup = Join-Path $outDir "sulsul-$version-windows-x64-setup.exe"
Get-Item $setup | Select-Object Name, @{ n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } }
