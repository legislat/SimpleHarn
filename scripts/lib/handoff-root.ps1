# Resolves the handoff layer root. PowerShell 5.1 compatible; dot-source this
# file from a script that runs at the repository root.
#
# Order: the SIMPLEHARN_HANDOFF environment variable, the sibling directory
# ..\SimpleHarn_handoff, then the legacy <repo>\handoff. Returns $null when none
# exists, so callers can fail loudly instead of reading an absent tree.

function Get-HandoffRoot {
  $fromEnv = $env:SIMPLEHARN_HANDOFF
  if ($fromEnv -and (Test-Path -LiteralPath $fromEnv -PathType Container)) {
    return (Resolve-Path -LiteralPath $fromEnv).Path
  }
  $sibling = Join-Path (Split-Path -Parent (Get-Location).Path) 'SimpleHarn_handoff'
  if (Test-Path -LiteralPath $sibling -PathType Container) {
    return (Resolve-Path -LiteralPath $sibling).Path
  }
  if (Test-Path -LiteralPath 'handoff' -PathType Container) {
    return (Resolve-Path -LiteralPath 'handoff').Path
  }
  return $null
}
