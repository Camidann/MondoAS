$ErrorActionPreference = "Stop"

$project = Split-Path -Parent $MyInvocation.MyCommand.Path
$exe = Join-Path $project "target\release\Mondo.exe"
$whisperBuild = Join-Path $project "src\whisper.cpp\build\bin\Release"
$modelCandidates = @(
    (Join-Path $project "src\whisper.cpp\models\ggml-base-q5_1.bin"),
    (Join-Path $project "ggml-base-q5_1.bin")
)
$model = $modelCandidates | Where-Object { Test-Path $_ -PathType Leaf } | Select-Object -First 1
$config = Join-Path $project "config.txt"
$faces = Join-Path $project "FACES"
$dist = Join-Path $project "dist\Mondo-portable"
$whisperDist = Join-Path $dist "whisper"

foreach ($required in @($exe, (Join-Path $whisperBuild "whisper-server.exe"), $config)) {
    if (-not (Test-Path $required -PathType Leaf)) {
        throw "No se encontró $required. Compila Mondo y whisper.cpp y descarga ggml-base-q5_1.bin primero."
    }
}
if (-not $model) {
    throw "No se encontró ggml-base-q5_1.bin. Descarga el modelo base-q5_1 de whisper.cpp primero."
}
if (-not (Test-Path $faces -PathType Container)) {
    throw "No se encontró la carpeta de caras: $faces"
}

New-Item -ItemType Directory -Force -Path $whisperDist | Out-Null
Copy-Item -Force $exe (Join-Path $dist "Mondo.exe")
Get-ChildItem $whisperBuild -File | Copy-Item -Destination $whisperDist -Force
Copy-Item -Force $model (Join-Path $whisperDist "ggml-base-q5_1.bin")
New-Item -ItemType Directory -Force -Path (Join-Path $dist "FACES") | Out-Null
Get-ChildItem $faces -File | Copy-Item -Destination (Join-Path $dist "FACES") -Force

$settings = Get-Content $config -Raw
$settings = [regex]::Replace($settings, '(?m)^whisper_server_bin=.*$', 'whisper_server_bin=whisper\whisper-server.exe')
$settings = [regex]::Replace($settings, '(?m)^whisper_model=.*$', 'whisper_model=whisper\ggml-base-q5_1.bin')
$avatarSetting = [regex]::Match($settings, '(?m)^avatar_file=(.*)$')
if ($avatarSetting.Success) {
    $avatarValue = $avatarSetting.Groups[1].Value.Trim()
    if ([System.IO.Path]::IsPathRooted($avatarValue)) {
        $avatarSource = $avatarValue
        $avatarRelative = [System.IO.Path]::GetFileName($avatarValue)
        $settings = [regex]::Replace($settings, '(?m)^avatar_file=.*$', "avatar_file=$avatarRelative")
    } else {
        $avatarSource = Join-Path $project $avatarValue
        $avatarRelative = $avatarValue
    }
    if (Test-Path $avatarSource -PathType Leaf) {
        $avatarDestination = Join-Path $dist $avatarRelative
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $avatarDestination) | Out-Null
        Copy-Item -Force $avatarSource $avatarDestination
    } else {
        Write-Warning "No se encontró la imagen configurada ($avatarSource); el personaje usará el avatar de respaldo."
    }
}
[System.IO.File]::WriteAllText(
    (Join-Path $dist "config.txt"),
    $settings,
    [System.Text.UTF8Encoding]::new($false)
)

$archive = Join-Path $project "dist\Mondo-portable.zip"
Compress-Archive -Path (Join-Path $dist "*") -DestinationPath $archive -Force
Write-Host "Paquete portable creado: $archive"
