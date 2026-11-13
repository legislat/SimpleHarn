# Seeded admission regression (TASK-ADMISSION-MOCK-RUNNER, ruling D4).
#
# Falsifier for the victim-release credit rule: in a disposable detached git
# worktree it flips ONE expression in src/scheduler/admission.rs - the
# `max(booking, measured)` release projection to `min` - and runs
# `cargo test --test admission_conformance` there. The run MUST exit non-zero:
# the conformance target must go red on that mutation or the credit pin is
# vacuous. The main working tree is never mutated: every file the run needs
# that is newer than HEAD (the conformance test is untracked) is COPIED INTO
# the worktree, never moved, and the worktree is removed at the end.
#
# Cardless: no engine starts, no model loads, no port binds. PowerShell 5.1
# compatible. Exit codes: 0 = regression proven (target went red),
# 1 = proof failed or setup error.

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path -LiteralPath (Join-Path $repoRoot 'Cargo.toml'))) {
  Write-Output 'FAIL :: could not resolve the repository root from the script location'
  exit 1
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$base = Join-Path $env:TEMP ('sh-seed-admission-' + $stamp)
$worktree = Join-Path $base 'wt'
$logPath = Join-Path $base 'seed-run.log'
$mutationCount = 0

function Remove-Worktree([string]$path) {
  git -C $repoRoot worktree remove --force $path 2>&1 | ForEach-Object { $_.ToString() } | ForEach-Object { Write-Output $_ }
  git -C $repoRoot worktree prune 2>&1 | Out-Null
}

try {
  New-Item -ItemType Directory -Path $base | Out-Null

  Write-Output ('RUN git worktree add --detach (HEAD) ' + $worktree)
  git -C $repoRoot worktree add --detach $worktree HEAD 2>&1 | ForEach-Object { $_.ToString() } | ForEach-Object { Write-Output $_ }
  if ($LASTEXITCODE -ne 0) { throw 'git worktree add failed' }

  # The conformance target is untracked; a bare worktree would not contain it.
  # Copy the current working copies of the test files so the worktree runs the
  # same code the main tree runs, without touching the main tree.
  $copyIn = @('tests\admission_conformance.rs', 'tests\mock_runtime.rs')
  foreach ($rel in $copyIn) {
    $src = Join-Path $repoRoot $rel
    if (Test-Path -LiteralPath $src) {
      Copy-Item -LiteralPath $src -Destination (Join-Path $worktree $rel) -Force
    }
  }

  # The seeded regression: credit the same-slot victim at the MINIMUM of its
  # booking and its measured footprint (the pre-ruling shape). The exact
  # expression lives in the victim release projection of finish_admit.
  $admissionPath = Join-Path $worktree 'src\scheduler\admission.rs'
  $text = Get-Content -LiteralPath $admissionPath -Raw
  $needle = '.max(o.live_measured_gb.unwrap_or(0.0))'
  $replacement = '.min(o.live_measured_gb.unwrap_or(0.0))'
  $mutationCount = ([regex]::Matches($text, [regex]::Escape($needle))).Count
  if ($mutationCount -ne 1) {
    throw ('expected exactly 1 occurrence of the credit expression, found ' + $mutationCount)
  }
  $patched = $text.Replace($needle, $replacement)
  if (-not $patched.Contains($replacement)) { throw 'mutation did not land' }
  Set-Content -LiteralPath $admissionPath -Value $patched -NoNewline
  Write-Output 'SEEDED :: victim release credit max(booking, measured) flipped to min'

  Write-Output 'RUN cargo test --test admission_conformance (in the worktree)'
  $env:CARGO_TARGET_DIR = (Join-Path $worktree 'target')
  Push-Location -LiteralPath $worktree
  try {
    $output = & cargo test --test admission_conformance 2>&1 | ForEach-Object { $_.ToString() }
    $code = $LASTEXITCODE
  } finally {
    Pop-Location
  }
  Set-Content -LiteralPath $logPath -Value ($output -join [Environment]::NewLine)
  foreach ($line in $output) {
    if ($line -match 'test result:|FAILED|failures:|panicked|error\[|seed') { Write-Output $line }
  }

  if ($code -ne 0) {
    Write-Output ('SEED PASS (non-zero exit ' + $code + ' on the credit-rule regression, as required)')
    Write-Output ('LOG ' + $logPath)
    Remove-Worktree $worktree
    exit 0
  }

  Write-Output 'SEED FAIL :: the conformance target stayed green on the mutation'
  Write-Output ('LOG ' + $logPath)
  Remove-Worktree $worktree
  exit 1
}
catch {
  Write-Output ('FAIL :: ' + $_.Exception.Message)
  if (Test-Path -LiteralPath $worktree) { Remove-Worktree $worktree }
  exit 1
}
