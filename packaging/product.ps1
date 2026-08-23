# Single source of truth for the packaging scripts. Dot-source it:
#   . (Join-Path $PSScriptRoot 'product.ps1')
$ProductName    = 'mouseutility'                 # exe / moniker / winget package name
$ProductDisplay = 'Mouse Utility'
$ProductRepo    = 'viorizz/mouseutility'                 # GitHub owner/repo
$ProductOwner   = 'viorizz'
$ProductDesc    = 'A terminal mouse utility for Windows: see your Logitech receivers and mice, battery, DPI and report rate from a beautiful TUI.'
$ProductTags    = @('battery', 'dpi', 'hidpp', 'logitech', 'mouse', 'rust', 'tui')
$ProductExe     = "$ProductName.exe"
$WingetId       = "$ProductOwner.$ProductName"
