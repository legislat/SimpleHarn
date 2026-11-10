# Release check for the GIT_WORKFLOW.md release rule. PowerShell 5.1 compatible.
# Read-only: it never tags, pushes, or modifies anything.
#
# Checks:
#   1. Cargo.toml declares a version.
#   2. A latest Git tag exists (a missing tag fails with a clear message).
#   3. The tag names the crate version. Accepted tag forms
#      (docs/process/GIT_WORKFLOW.md, "Tag and version naming"):
#      v<major>.<minor>.<patch> or v<major>.<minor>.<patch>.<hotfix>. The
#      crate version stays three-part semver; the tag's first three segments
#      must equal it, so v1.5.1.1 accompanies crate 1.5.1. With -Prepare the
#      tag itself is not matched: a newer crate version prepares a normal
#      release, and a crate version equal to the tag core prepares a hotfix
#      that keeps the crate version.
#   4. CHANGELOG.md contains the version string.
#   5. README.md names the latest tag, or the prepared version with -Prepare.
#   6. No known stale phrase appears in current durable docs or source
#      (archive paths excluded).
#   7. `cargo doc --no-deps` is warning-free (skip with -SkipDoc).
#   8. The webui status fixture quotes the same version as Cargo.toml (a bump
#      that forgets the fixture pin ships a red test; v1.1.3 proved the gap).
#   9. Public docs carry no machine paths (README.md, CHANGELOG.md,
#      CONTRIBUTING.md, AGENTS.md, docs/**). The ban targets personal
#      locations on the operator machine; the author-machine root is banned
#      whole - the harness ships no default under it (FR-15).
#      Rule home: docs/process/GIT_WORKFLOW.md (release section).
#  10. Release facts agree across Cargo.toml, CHANGELOG.md, README.md,
#      docs/reference/COMPATIBILITY.md, the webui fixture, and
#      scripts/make-public-asset.ps1 (node scripts/check-doc-facts.mjs).
#
# Exit 0 only when every check passes.
#
# Test seam: -CompareTag <tag> runs only the check 3 comparison against the
# Cargo.toml version, prints the verdict, and exits: 0 on a match, 1 with the
# exact accepted tag forms. It reads no tag from Git and changes nothing.

param(
  [switch]$Prepare,
  [switch]$SkipDoc,
  [string]$CompareTag
)

$ErrorActionPreference = 'Continue'

$script:allPass = $true

function Report([bool]$ok, [string]$message) {
  if ($ok) { Write-Output ("PASS " + $message) }
  else { Write-Output ("FAIL " + $message); $script:allPass = $false }
}

function Get-VersionKey([string]$v) {
  $parts = ($v -replace '^v', '') -split '\.'
  $nums = @(0, 0, 0)
  for ($i = 0; $i -lt 3 -and $i -lt $parts.Count; $i++) {
    $n = 0
    if ([int]::TryParse(($parts[$i] -replace '[^0-9].*$', ''), [ref]$n)) { $nums[$i] = $n }
  }
  return $nums[0] * 1000000 + $nums[1] * 1000 + $nums[2]
}

# Tag grammar of the naming rule (docs/process/GIT_WORKFLOW.md): a tag is
# v<ma>.<mi>.<pa> or v<ma>.<mi>.<pa>.<hotfix>, the hotfix being a positive
# integer (a fourth segment of 0 is refused). Returns $null for any other
# shape. A crate version parses through the same seam with a synthetic v.
function Get-TagParts([string]$tag) {
  $m = [regex]::Match(("$tag"), '^v(\d+)\.(\d+)\.(\d+)(?:\.([1-9]\d*))?$')
  if (-not $m.Success) { return $null }
  $hotfix = $null
  if ($m.Groups[4].Success) { $hotfix = [int]$m.Groups[4].Value }
  return [pscustomobject]@{
    Major  = [int]$m.Groups[1].Value
    Minor  = [int]$m.Groups[2].Value
    Patch  = [int]$m.Groups[3].Value
    Hotfix = $hotfix
  }
}

# The one comparison behind check 3 and the -CompareTag seam. Ok when the
# tag's first three segments equal the three-part crate version; a fourth
# tag segment only marks the hotfix form. Kind: exact, hotfix, bad-tag,
# bad-version, or mismatch. Version is the bare Cargo.toml version (no v).
function Compare-ReleaseTag([string]$tag, [string]$version) {
  $vp = Get-TagParts ('v' + $version)
  if ($null -eq $vp) {
    return [pscustomobject]@{ Ok = $false; Kind = 'bad-version'; Message =
      ("Cargo.toml version " + $version + " is not three-part semver X.Y.Z") }
  }
  $tp = Get-TagParts $tag
  if ($null -eq $tp) {
    return [pscustomobject]@{ Ok = $false; Kind = 'bad-tag'; Message =
      ("tag " + $tag + " is neither v" + $version + " nor v" + $version + ".<hotfix>") }
  }
  if ($tp.Major -ne $vp.Major -or $tp.Minor -ne $vp.Minor -or $tp.Patch -ne $vp.Patch) {
    return [pscustomobject]@{ Ok = $false; Kind = 'mismatch'; Message =
      ("tag " + $tag + " names " + $tp.Major + "." + $tp.Minor + "." + $tp.Patch +
       ", the crate version is " + $version) }
  }
  $kind = 'exact'
  if ($null -ne $tp.Hotfix) { $kind = 'hotfix' }
  return [pscustomobject]@{ Ok = $true; Kind = $kind; Message =
    ("tag " + $tag + " matches crate version " + $version +
     $(if ($kind -eq 'hotfix') { ' (hotfix form)' } else { '' })) }
}

function Format-AcceptedTagForms([string]$version) {
  $v = if ($version) { $version } else { '<major>.<minor>.<patch>' }
  return "accepted forms: v" + $v + " or v" + $v + ".<hotfix> " +
    "(<hotfix> is a positive integer, for example v" + $v + ".1)"
}

# 1. Version in Cargo.toml.
$version = $null
if (Test-Path 'Cargo.toml') {
  $m = Select-String -Path 'Cargo.toml' -Pattern '^\s*version\s*=\s*"([^"]+)"' |
    Select-Object -First 1
  if ($m) { $version = $m.Matches[0].Groups[1].Value }
}
Report ([bool]$version) ("Cargo.toml declares a version" + $(if ($version) { ": " + $version } else { "" }))

# Test seam: run the check 3 comparison alone against Cargo.toml, then exit.
# It never reads a tag from Git and changes nothing.
if ($CompareTag) {
  if (-not $version) {
    Write-Output ("FAIL cannot compare tag " + $CompareTag + ": Cargo.toml declares no version")
    exit 1
  }
  $seam = Compare-ReleaseTag $CompareTag $version
  if ($seam.Ok) {
    Write-Output ("PASS " + $seam.Message)
    exit 0
  }
  Write-Output ("FAIL " + $seam.Message)
  Write-Output ("  " + (Format-AcceptedTagForms $version))
  exit 1
}

# 2. Latest tag.
$tag = $null
if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
  Report $false "git not on PATH"
} else {
  $global:LASTEXITCODE = -1
  $tagOut = git describe --tags --abbrev=0 2>$null
  $tagCode = $LASTEXITCODE
  if ($tagCode -eq 0 -and $tagOut) { $tag = ("$tagOut").Trim() }
}
Report ([bool]$tag) $(if ($tag) {
  "latest tag found: " + $tag
} else {
  "no Git tag found; a release needs an annotated tag v<version> or v<version>.<hotfix> (run git tag -a v<version>)"
})

if (-not $version -or -not $tag) {
  Write-Output "RELEASE CHECK FAIL (version or tag missing; later checks skipped)"
  exit 1
}

# 3. The tag names the crate version, or the tree prepares a new release.
#    Accepted tag forms: v<X>.<Y>.<Z> or v<X>.<Y>.<Z>.<hotfix>; the first
#    three segments must equal the three-part crate version (see the header
#    and docs/process/GIT_WORKFLOW.md). With -Prepare the tag itself is not
#    matched: a newer crate version prepares a normal release; a crate
#    version equal to the tag core prepares a hotfix on this crate version.
if ($Prepare) {
  $vkey = Get-VersionKey $version
  $tkey = Get-VersionKey $tag
  if ($vkey -gt $tkey) {
    Report $true ("preparing a new release: version " + $version + " exceeds tag " + $tag)
  } elseif ($vkey -eq $tkey) {
    Report $true ("preparing a hotfix: crate version " + $version +
      " equals the tag core of " + $tag + "; this is read as hotfix intent, not as a" +
      " forgotten Cargo.toml bump - if a new release was intended, bump the crate" +
      " version first; the new tag is v" + $version + ".<hotfix>")
  } else {
    Report $false ("preparing a release needs a version above tag " + $tag +
      ", or an equal core to prepare a hotfix")
  }
} else {
  $cmp = Compare-ReleaseTag $tag $version
  Report $cmp.Ok ("latest tag " + $cmp.Message +
    $(if ($cmp.Ok) { "" } else { "; " + (Format-AcceptedTagForms $version) }))
}

# 4. Changelog entry for the version.
$changelogOk = (Test-Path 'CHANGELOG.md') -and
  (Select-String -Path 'CHANGELOG.md' -Pattern $version -SimpleMatch -Quiet)
Report $changelogOk ("CHANGELOG.md contains version " + $version)

# 5. README release claim names the tag, or the prepared version with -Prepare.
$readmeTag = if ($Prepare) { "v" + $version } else { $tag }
$readmeOk = (Test-Path 'README.md') -and
  (Select-String -Path 'README.md' -Pattern $readmeTag -SimpleMatch -Quiet)
Report $readmeOk ("README.md names tag " + $readmeTag)

# 6. Known stale phrases in current durable docs and source.
$stalePhrases = @("there is no auth", "arrive with their own turns")
$scanRoots = @("README.md", "AGENTS.md", "CONTRIBUTING.md", "docs/DOCS_README.md", "docs/process", "docs/architecture", "docs/reference", "src", "tests")
$scanFiles = @()
foreach ($root in $scanRoots) {
  if (Test-Path -LiteralPath $root) {
    if ((Get-Item -LiteralPath $root).PSIsContainer) {
      $scanFiles += (Get-ChildItem -LiteralPath $root -Recurse -File |
        Where-Object { ($_.Extension -eq '.md' -or $_.Extension -eq '.rs') -and
          $_.FullName -notmatch '\\archive\\' } |
        ForEach-Object { $_.FullName })
    } else {
      $scanFiles += (Resolve-Path -LiteralPath $root).Path
    }
  }
}
$staleHits = @()
foreach ($phrase in $stalePhrases) {
  $hit = Select-String -Path $scanFiles -Pattern $phrase -SimpleMatch -List -ErrorAction SilentlyContinue
  if ($hit) { $staleHits += ($phrase + " -> " + $hit.Path) }
}
$staleOk = ($staleHits.Count -eq 0)
Report $staleOk ("no known stale phrase in current docs and source" +
  $(if ($staleHits.Count -gt 0) { ": " + ($staleHits -join "; ") } else { "" }))

# 7. cargo doc warnings.
if (-not $SkipDoc) {
  if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Report $false "cargo not on PATH"
  } else {
    Write-Output "Running cargo doc --no-deps"
    $global:LASTEXITCODE = -1
    $docOutput = & cargo doc --no-deps 2>&1 | Out-String
    $docCode = $LASTEXITCODE
    $docOk = ($docCode -eq 0) -and ($docOutput -notmatch 'warning:')
    Report $docOk "cargo doc --no-deps is warning-free"
    if (-not $docOk) {
      $warningCount = ([regex]::Matches($docOutput, 'warning:')).Count
      Write-Output ("cargo doc warning count: " + $warningCount)
    }
  }
}

# 8. The webui fixture quotes the crate version in its TOP-LEVEL `version`
#    property, compared case-sensitively - exactly the key the parity test
#    `the_webui_fixture_is_a_document_the_endpoint_can_actually_emit` reads
#    (serde exact-key lookup). A first-regex-match read would compare a nested
#    decoy or a recased key instead (review B3R-01). The release-check never
#    runs tests, so the bump commit itself needs this mechanical guard.
$fixturePath = 'tests/webui/fixtures/status.json'
$fixtureOk = $false
if (Test-Path -LiteralPath $fixturePath) {
  $fixtureVersion = $null
  $fixtureError = $null
  try {
    $fixture = Get-Content -LiteralPath $fixturePath -Raw -Encoding UTF8 | ConvertFrom-Json
    $prop = @($fixture.PSObject.Properties) |
      Where-Object { $_.Name -ceq 'version' } |
      Select-Object -First 1
    if ($null -ne $prop) { $fixtureVersion = $prop.Value }
  } catch {
    $fixtureError = "$_"
  }
  if ($fixtureError) {
    Report $false ("webui fixture is not valid JSON: " + $fixtureError)
  } elseif ($null -eq $fixtureVersion) {
    Report $false ("webui fixture has no top-level version property: " + $fixturePath)
  } else {
    $fixtureOk = ($fixtureVersion -ceq $version)
    Report $fixtureOk ("webui fixture quotes top-level version " + $fixtureVersion +
      " matching Cargo.toml " + $version)
  }
} else {
  Report $false "webui fixture missing: $fixturePath"
}

# 9. Public docs carry no personal machine paths (operator ruling 2026-11-04,
#    review follow-up: DECISIONS.md and DOCTRINE.md are scanned like every
#    other live doc; the ledger line that leaked a research path was scrubbed
#    to a name-only reference). The ban set is the shared constant
#    scripts/machine-path-bans.json - node scripts/check-docs.mjs enforces the
#    same patterns at every push, so a durable doc cannot pass the cardless
#    gate and trip only this release leg on the same path. The blocklist names
#    personal locations on the operator machine: the repo roots, user homes,
#    the scratch and backup volumes, and the staging and rollback directory
#    names. Since FR-15 (operator ruling 2026-11-06) the author-machine root
#    is banned with no exception: the harness ships no engine default under
#    it, so no durable doc has a reason to name it.
$bansPath = Join-Path $PSScriptRoot 'machine-path-bans.json'
$bansError = ''
$personalPathPatterns = @()
try {
  $personalPathPatterns = @(
    (Get-Content -LiteralPath $bansPath -Raw -Encoding UTF8 |
      ConvertFrom-Json).bans)
} catch {
  $bansError = "$_"
}
$personalPathPatterns = @($personalPathPatterns |
  Where-Object { ($_ -is [string]) -and ($_ -ne '') })
if ($bansError -or $personalPathPatterns.Count -eq 0) {
  Report $false ("shared machine-path ban set missing or empty: " + $bansPath +
    $(if ($bansError) { " (" + $bansError + ")" } else { "" }))
} else {
  $publicDocs = @('README.md', 'CHANGELOG.md', 'CONTRIBUTING.md', 'AGENTS.md')
  if (Test-Path 'docs') {
    $publicDocs += (Get-ChildItem -LiteralPath 'docs' -Recurse -Filter '*.md' |
      ForEach-Object { $_.FullName })
  }
  $machinePathHits = @()
  foreach ($pattern in $personalPathPatterns) {
    $hits = Select-String -Path $publicDocs -Pattern $pattern -ErrorAction SilentlyContinue
    foreach ($hit in $hits) {
      $machinePathHits += ($hit.Path + ":" + $hit.LineNumber)
    }
  }
  Report ($machinePathHits.Count -eq 0) ("public docs carry no machine paths" +
    $(if ($machinePathHits.Count -gt 0) { ": " + (($machinePathHits | Sort-Object -Unique) -join "; ") } else { "" }))
}

# 10. Release facts agree across the durable documents and the asset script.
#     The tool prints each FAIL with the file and the expected/actual values;
#     its output is reported verbatim. Review F-02 and F-05: shape-only checks
#     passed while the release facts were wrong, so this leg pins the facts.
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
  Report $false "node not on PATH (scripts/check-doc-facts.mjs cannot run)"
} else {
  $global:LASTEXITCODE = -1
  $factsOutput = (& node (Join-Path $PSScriptRoot 'check-doc-facts.mjs') 2>&1 | Out-String).Trim()
  $factsCode = $LASTEXITCODE
  if ($factsOutput) { Write-Output $factsOutput }
  Report ($factsCode -eq 0) "release facts agree (node scripts/check-doc-facts.mjs)"
}

if ($script:allPass) {
  Write-Output "RELEASE CHECK PASS"
  exit 0
}
Write-Output "RELEASE CHECK FAIL"
exit 1
