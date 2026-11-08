# Instala (o desinstala) el comando "buscatxt" en Windows, solo para el
# usuario actual (no requiere permisos de administrador).
#
# Uso (en PowerShell, desde la carpeta del proyecto):
#   powershell -ExecutionPolicy Bypass -File scripts\instalar-windows.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\instalar-windows.ps1 -Desinstalar
#
# O con doble clic en scripts\instalar-windows.bat / desinstalar-windows.bat.
#
# El script no descarga nada: copia el ejecutable incluido en dist\ (o el
# compilado con "cargo build --release" si no existe) a
# %LOCALAPPDATA%\Programs\buscatxt y agrega esa carpeta al PATH del usuario.

param(
    [switch]$Desinstalar
)

$ErrorActionPreference = 'Stop'

$Name = 'buscatxt.exe'
$Dest = Join-Path $env:LOCALAPPDATA 'Programs\buscatxt'
$Root = Split-Path -Parent $PSScriptRoot

# PATH del usuario tal como esta guardado en el registro (sin expandir las
# variables como %USERPROFILE%, para no alterar las demas entradas).
function Get-UserPath {
    $key = Get-Item -Path 'HKCU:\Environment'
    return [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
}

function Set-UserPath([string]$Value) {
    Set-ItemProperty -Path 'HKCU:\Environment' -Name 'Path' -Value $Value -Type ExpandString
    # Avisa a Windows del cambio para que las nuevas terminales lo vean:
    # crear y borrar una variable de usuario envia ese aviso.
    [Environment]::SetEnvironmentVariable('BUSCATXT_ACTUALIZAR', '1', 'User')
    [Environment]::SetEnvironmentVariable('BUSCATXT_ACTUALIZAR', $null, 'User')
}

function Test-SameFolder([string]$A, [string]$B) {
    $a = [Environment]::ExpandEnvironmentVariables($A).Trim().TrimEnd('\')
    $b = [Environment]::ExpandEnvironmentVariables($B).Trim().TrimEnd('\')
    return [string]::Equals($a, $b, [StringComparison]::OrdinalIgnoreCase)
}

function Find-Binary {
    $candidates = @(
        (Join-Path $Root 'dist\buscatxt-windows-x86_64.exe'),
        (Join-Path $Root 'target\release\buscatxt.exe')
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    throw "No se encontro buscatxt.exe en dist\ ni en target\release\. Compilalo con: cargo build --release"
}

function Install-Command {
    $source = Find-Binary
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    $target = Join-Path $Dest $Name
    Copy-Item -LiteralPath $source -Destination $target -Force
    $version = & $target -V
    Write-Host "Instalado: $target ($version)"

    $entries = (Get-UserPath) -split ';' | Where-Object { $_.Trim() -ne '' }
    $present = $entries | Where-Object { Test-SameFolder $_ $Dest }
    if (-not $present) {
        Set-UserPath ((@($entries) + $Dest) -join ';')
        Write-Host "Se agrego $Dest al PATH del usuario."
    }
    # Tambien en esta ventana, para poder probarlo ya.
    if (-not (($env:Path -split ';') | Where-Object { Test-SameFolder $_ $Dest })) {
        $env:Path = "$env:Path;$Dest"
    }

    Write-Host ''
    Write-Host 'Abre una ventana nueva de PowerShell o cmd y prueba:  buscatxt --help'
}

function Uninstall-Command {
    $target = Join-Path $Dest $Name
    if (Test-Path -LiteralPath $Dest) {
        Remove-Item -LiteralPath $Dest -Recurse -Force
        Write-Host "Eliminado: $Dest"
    } else {
        Write-Host 'buscatxt no estaba instalado.'
    }
    $entries = (Get-UserPath) -split ';' | Where-Object { $_.Trim() -ne '' }
    $kept = $entries | Where-Object { -not (Test-SameFolder $_ $Dest) }
    if (@($kept).Count -ne @($entries).Count) {
        Set-UserPath (@($kept) -join ';')
        Write-Host "Se quito $Dest del PATH del usuario."
    }
}

if ($Desinstalar) {
    Uninstall-Command
} else {
    Install-Command
}
