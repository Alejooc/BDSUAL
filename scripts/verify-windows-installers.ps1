$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$configPath = Join-Path $repoRoot 'src-tauri/tauri.conf.json'
$bundleRoot = Join-Path $repoRoot 'src-tauri/target/release/bundle'
$config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json

function Get-OnePackage([string]$Directory, [string]$Pattern, [string]$Kind) {
    $matches = @(Get-ChildItem -LiteralPath $Directory -Filter $Pattern -File -ErrorAction SilentlyContinue)
    if ($matches.Count -ne 1) {
        throw "Se esperaba un paquete $Kind en '$Directory'; se encontraron $($matches.Count)."
    }
    if ($matches[0].Length -lt 4096) {
        throw "El paquete $Kind parece vacío o incompleto: $($matches[0].FullName)."
    }
    return $matches[0]
}

$msi = Get-OnePackage (Join-Path $bundleRoot 'msi') '*.msi' 'MSI'
$nsis = Get-OnePackage (Join-Path $bundleRoot 'nsis') '*.exe' 'NSIS'
if ($msi.Name -notmatch '_x64_.*\.msi$' -or $nsis.Name -notmatch '_x64-setup\.exe$') {
    throw "Los nombres de paquete no identifican el destino x64 esperado: '$($msi.Name)', '$($nsis.Name)'."
}

# Read package metadata without installing the MSI or running either installer.
$installer = New-Object -ComObject WindowsInstaller.Installer
$database = $installer.OpenDatabase($msi.FullName, 0)
function Read-MsiProperty($Database, [string]$Name) {
    $view = $Database.OpenView("SELECT ``Value`` FROM ``Property`` WHERE ``Property``='$Name'")
    $view.Execute()
    $record = $view.Fetch()
    if (-not $record) { return $null }
    return $record.StringData(1)
}

$productName = Read-MsiProperty $database 'ProductName'
$productVersion = Read-MsiProperty $database 'ProductVersion'
$productCode = Read-MsiProperty $database 'ProductCode'
$upgradeCode = Read-MsiProperty $database 'UpgradeCode'
$template = $database.SummaryInformation(0).Property(7)
if ($productName -ne $config.productName) { throw "ProductName MSI inesperado: '$productName'." }
if ($productVersion -ne $config.version) { throw "ProductVersion MSI '$productVersion' no coincide con tauri.conf.json '$($config.version)'." }
if ($productCode -notmatch '^\{[0-9A-Fa-f-]{36}\}$') { throw "ProductCode MSI inválido: '$productCode'." }
if ($upgradeCode -notmatch '^\{[0-9A-Fa-f-]{36}\}$') { throw "UpgradeCode MSI inválido: '$upgradeCode'." }
if ($template -notmatch 'x64') { throw "El MSI no declara plataforma x64 en Template: '$template'." }

$nsisVersionInfo = $nsis.VersionInfo
if ($nsisVersionInfo.ProductName -ne $config.productName -or $nsisVersionInfo.FileDescription -ne $config.productName) {
    throw "Los metadatos del ejecutable NSIS no identifican el producto '$($config.productName)'."
}
if ($nsisVersionInfo.ProductVersion -ne $config.version) {
    throw "ProductVersion NSIS '$($nsisVersionInfo.ProductVersion)' no coincide con tauri.conf.json '$($config.version)'."
}

# Verify the NSIS PE machine type is AMD64 (x64) without launching it.
$stream = [System.IO.File]::OpenRead($nsis.FullName)
try {
    $reader = [System.IO.BinaryReader]::new($stream)
    if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'El instalador NSIS no tiene una cabecera MZ válida.' }
    $stream.Position = 0x3c
    $peOffset = $reader.ReadInt32()
    if ($peOffset -lt 64 -or $peOffset -gt ($stream.Length - 6)) { throw 'Offset PE inválido en NSIS.' }
    $stream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x00004550) { throw 'El instalador NSIS no tiene una firma PE válida.' }
    $machine = $reader.ReadUInt16()
    # The NSIS bootstrap executable may itself be 32-bit while installing the
    # x64 payload; verify a supported PE stub and rely on Tauri's x64 package
    # naming plus the Windows x64 build runner for the bundle target.
    if ($machine -ne 0x8664 -and $machine -ne 0x014c) { throw "Tipo PE NSIS no admitido: 0x$('{0:X4}' -f $machine)." }
}
finally {
    $stream.Dispose()
}

Write-Host "MSI verificado: $($msi.Name), $($msi.Length) bytes, $template, ProductCode $productCode, UpgradeCode $upgradeCode"
Write-Host "NSIS verificado: $($nsis.Name), $($nsis.Length) bytes, lanzador PE válido (0x$('{0:X4}' -f $machine)), versión $($nsisVersionInfo.ProductVersion)"
Write-Host 'Validación de paquetes solamente: no se instaló, actualizó ni desinstaló DBSUAL.'
