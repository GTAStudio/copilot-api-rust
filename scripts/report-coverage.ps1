param(
    [string]$Executable,
    [string]$ProfileData,
    [string]$SourceRoot,
    [string]$OutputPath,
    [ValidateRange(0, 100)]
    [double]$MinimumPercent = 0,
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Read-LcovRecords([string[]]$Content) {
    $records = [System.Collections.Generic.List[object]]::new()
    $current = $null
    foreach ($line in $Content) {
        if ($line.StartsWith('SF:')) {
            if ($null -ne $current) { throw 'Unterminated LCOV record' }
            $current = [pscustomobject]@{
                File = $line.Substring(3)
                Lines = [System.Collections.Generic.SortedDictionary[int,long]]::new()
                Found = -1
                Hit = -1
            }
        } elseif ($line -match '^DA:(\d+),(\d+)(?:,.*)?$') {
            if ($null -eq $current) { throw 'LCOV line without source file' }
            $number = [int]$Matches[1]
            if ($current.Lines.ContainsKey($number)) { throw 'Duplicate LCOV source line' }
            $current.Lines.Add($number, [long]$Matches[2])
        } elseif ($line -match '^LF:(\d+)$') {
            $current.Found = [int]$Matches[1]
        } elseif ($line -match '^LH:(\d+)$') {
            $current.Hit = [int]$Matches[1]
        } elseif ($line -eq 'end_of_record') {
            if ($null -eq $current) { throw 'Unexpected LCOV record end' }
            $hit = @($current.Lines.Values | Where-Object { $_ -gt 0 }).Count
            if ($current.Lines.Count -gt $current.Found -or $hit -gt $current.Hit -or
                ($current.Hit - $hit) -gt ($current.Found - $current.Lines.Count)) {
                throw "LLVM line totals do not match source records: $($current.File)"
            }
            $records.Add($current)
            $current = $null
        }
    }
    if ($null -ne $current) { throw 'Incomplete LCOV export' }
    return ,($records.ToArray())
}

function Measure-CoveredLines($Lines, [object[]]$Exclusions) {
    $total = 0
    $covered = 0
    $excluded = 0
    foreach ($number in $Lines.Keys) {
        $skip = $false
        foreach ($range in $Exclusions) {
            if ($number -ge $range.Start -and $number -le $range.End) { $skip = $true; break }
        }
        if ($skip) { $excluded++; continue }
        $total++
        if ($Lines[$number] -gt 0) { $covered++ }
    }
    return [pscustomobject]@{ Total = $total; Covered = $covered; Excluded = $excluded }
}

function Get-TestExclusions([string]$Source, [object[]]$Functions, [string]$TestModule = '') {
    $ranges = [System.Collections.Generic.List[object]]::new()
    $lineCount = ($Source -split "`n").Count
    if ($TestModule) {
        $modulePattern = '^<?[A-Za-z0-9_]+::' + [regex]::Escape($TestModule) + '::'
        if ($Functions.Count -eq 0 -or @($Functions | Where-Object { $_.Name -notmatch $modulePattern }).Count -gt 0) {
            throw 'Integration test file contains a non-test function mapping'
        }
        $ranges.Add([pscustomobject]@{ Start = 1; End = $lineCount; Reason = "cfg(test) external module $TestModule verified by Rust symbols" })
        return ,($ranges.ToArray())
    }
    $modules = [regex]::Matches($Source, '(?m)^#\[cfg\(test\)\]\r?\nmod tests\s*\{')
    if ($modules.Count -gt 1) { throw 'Multiple test modules require explicit coverage classification' }
    foreach ($module in $modules) {
        $start = ($Source.Substring(0, $module.Index) -split "`n").Count
        $tests = @($Functions | Where-Object {
            $_.Start -ge $start -and $_.Name -match '^<?(?:[A-Za-z0-9_]+::)*tests::'
        })
        if ($tests.Count -eq 0) {
            throw 'No verified Rust test function mappings for cfg(test) module'
        }
        foreach ($test in $tests) {
            $ranges.Add([pscustomobject]@{
                Start = $test.Start; End = $test.End; Reason = 'cfg(test) function verified by Rust symbol'
            })
        }
    }
    $helpers = [regex]::Matches($Source, '(?m)^#\[cfg\(test\)\]\r?\n(?<declaration>(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(?<name>[A-Za-z_][A-Za-z0-9_]*))')
    foreach ($helper in $helpers) {
        $start = ($Source.Substring(0, $helper.Groups['declaration'].Index) -split "`n").Count
        $name = [regex]::Escape($helper.Groups['name'].Value)
        $mapped = @($Functions | Where-Object { $_.Start -eq $start -and $_.Name -match "::$name(?=::|$|<)" })
        if ($mapped.Count -eq 0) { throw "Cannot verify cfg(test) helper: $name" }
        $end = ($mapped | Measure-Object -Property End -Maximum).Maximum
        $ranges.Add([pscustomobject]@{ Start = $start; End = [int]$end; Reason = "cfg(test) helper $name" })
    }
    return ,($ranges.ToArray())
}

function Assert-CoverageEqual($Actual, $Expected, [string]$Message) {
    if ($Actual -ne $Expected) { throw "$Message (actual=$Actual expected=$Expected)" }
}

if ($SelfTest) {
    $fixture = @(
        'SF:fixture.rs', 'DA:1,1', 'DA:2,0', 'DA:4,3', 'DA:8,2',
        'LF:4', 'LH:3', 'end_of_record'
    )
    $records = Read-LcovRecords $fixture
    Assert-CoverageEqual $records.Count 1 'One source record'
    $summary = Measure-CoveredLines $records[0].Lines @(@{ Start = 8; End = 10 })
    Assert-CoverageEqual $summary.Total 3 'Excluded test lines do not affect production total'
    Assert-CoverageEqual $summary.Covered 2 'Uncovered production lines remain in denominator'
    Assert-CoverageEqual $summary.Excluded 1 'Only the test range is excluded'
    $summary = Measure-CoveredLines $records[0].Lines @()
    Assert-CoverageEqual $summary.Total 4 'Raw line total is preserved'
    $rejected = $false
    try { Read-LcovRecords @('SF:fixture.rs', 'DA:1,1', 'LF:0', 'LH:0', 'end_of_record') | Out-Null }
    catch { $rejected = $true }
    Assert-CoverageEqual $rejected $true 'Inconsistent LLVM totals must fail'
    $overlap = Read-LcovRecords @('SF:fixture.rs', 'FN:1,outer', 'FN:1,closure', 'DA:1,2', 'LF:2', 'LH:2', 'end_of_record')
    $summary = Measure-CoveredLines $overlap[0].Lines @()
    Assert-CoverageEqual $summary.Total 1 'Async wrapper overlap does not duplicate a physical source line'
    Assert-CoverageEqual $overlap[0].Found 2 'Original LLVM mapping total remains available'
    $source = "fn production() {}`n#[cfg(test)]`nmod tests {`nfn check() {}`n}`n"
    $functions = @(
        [pscustomobject]@{ Name = 'fixture::production'; Start = 1; End = 1 },
        [pscustomobject]@{ Name = 'fixture::tests::check'; Start = 4; End = 4 }
    )
    $ranges = Get-TestExclusions $source $functions
    Assert-CoverageEqual $ranges[0].Start 4 'Only the mapped test function is excluded'
    $functions += [pscustomobject]@{ Name = 'fixture::production_after_tests'; Start = 6; End = 6 }
    $functions += [pscustomobject]@{ Name = 'fixture::production_generic::<fixture::tests::Fixture>'; Start = 7; End = 7 }
    $ranges = Get-TestExclusions $source $functions
    $summary = Measure-CoveredLines @{ 1 = 1; 4 = 1; 6 = 0; 7 = 0 } $ranges
    Assert-CoverageEqual $summary.Total 3 'Production functions after tests and test-type instantiations are retained'
    Assert-CoverageEqual $summary.Covered 1 'Uncovered production code after tests is retained'
    $ranges = Get-TestExclusions 'fn render_test() {}' @([pscustomobject]@{ Name = 'fixture::ui_tests::render_test'; Start = 1; End = 1 }) -TestModule 'ui_tests'
    Assert-CoverageEqual $ranges[0].End 1 'Verified external test module is excluded'
    $rejected = $false
    try { Get-TestExclusions 'fn production() {}' @([pscustomobject]@{ Name = 'fixture::production'; Start = 1; End = 1 }) -TestModule 'ui_tests' | Out-Null } catch { $rejected = $true }
    Assert-CoverageEqual $rejected $true 'External test modules cannot hide production symbols'
    Write-Output 'COVERAGE_REPORT_SELF_TEST_PASSED'
    return
}

foreach ($required in @($Executable, $ProfileData, $SourceRoot, $OutputPath)) {
    if ([string]::IsNullOrWhiteSpace($required)) { throw 'Executable, Profile, SourceRoot and OutputPath are required' }
}
$executablePath = (Resolve-Path -LiteralPath $Executable).Path
$profilePath = (Resolve-Path -LiteralPath $ProfileData).Path
$sourcePath = (Resolve-Path -LiteralPath $SourceRoot).Path.TrimEnd([char[]]'/\') + [IO.Path]::DirectorySeparatorChar
$rootSource = [IO.File]::ReadAllText((Join-Path $sourcePath 'main.rs'))
$testModules = @([regex]::Matches($rootSource, '(?m)^#\[cfg\((?:test|all\(test,\s*debug_assertions\))\)\]\r?\nmod (?<name>[A-Za-z_][A-Za-z0-9_]*);') | ForEach-Object { $_.Groups['name'].Value })
$output = [IO.Path]::GetFullPath($OutputPath)
$llvm = (Get-Command llvm-cov -ErrorAction Stop).Source
$demangler = (Get-Command llvm-cxxfilt -ErrorAction Stop).Source
$export = (& $llvm export $executablePath "-instr-profile=$profilePath" | Out-String) | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'LLVM JSON export failed' }
$lcov = @(& $llvm export $executablePath "-instr-profile=$profilePath" -format=lcov)
if ($LASTEXITCODE -ne 0) { throw 'LLVM LCOV export failed' }
$records = Read-LcovRecords $lcov
$functionMaps = @($export.data[0].functions)
$names = @($functionMaps | ForEach-Object { $_.name } | & $demangler)
if ($LASTEXITCODE -ne 0 -or $names.Count -ne $functionMaps.Count) { throw 'Rust symbol decoding failed' }
$functionsByFile = @{}
for ($index = 0; $index -lt $functionMaps.Count; $index++) {
    $function = $functionMaps[$index]
    $file = [IO.Path]::GetFullPath($function.filenames[0])
    if (-not $file.StartsWith($sourcePath, [StringComparison]::OrdinalIgnoreCase)) { continue }
    $regions = @($function.regions | Where-Object { $_[5] -eq 0 -and $_[7] -eq 0 })
    if ($regions.Count -eq 0) { continue }
    $start = ($regions | ForEach-Object { $_[0] } | Measure-Object -Minimum).Minimum
    $end = ($regions | ForEach-Object { $_[2] } | Measure-Object -Maximum).Maximum
    if (-not $functionsByFile.ContainsKey($file)) { $functionsByFile[$file] = [System.Collections.Generic.List[object]]::new() }
    $functionsByFile[$file].Add([pscustomobject]@{ Name = $names[$index]; Start = [int]$start; End = [int]$end })
}
$rows = [System.Collections.Generic.List[object]]::new()
$mappedFiles = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($record in $records) {
    $file = [IO.Path]::GetFullPath($record.File)
    if (-not $file.StartsWith($sourcePath, [StringComparison]::OrdinalIgnoreCase)) { continue }
    if (-not $functionsByFile.ContainsKey($file)) { throw "Missing Rust function mappings: $file" }
    $source = [IO.File]::ReadAllText($file)
    $moduleName = [IO.Path]::GetFileNameWithoutExtension($file)
    $testModule = if ($testModules -contains $moduleName -and [IO.Path]::GetDirectoryName($file) -eq $sourcePath.TrimEnd([char[]]'/\')) { $moduleName } else { '' }
    $ranges = Get-TestExclusions $source ($functionsByFile[$file].ToArray()) -TestModule $testModule
    $summary = Measure-CoveredLines $record.Lines $ranges
    $rows.Add([pscustomobject]@{
        File = $file.Substring($sourcePath.Length).Replace('\', '/')
        RawLines = $record.Lines.Count
        RawCovered = @($record.Lines.Values | Where-Object { $_ -gt 0 }).Count
        LlvmDeclaredLines = $record.Found
        LlvmDeclaredCovered = $record.Hit
        OverlappingMappedLines = $record.Found - $record.Lines.Count
        ProductionLines = $summary.Total
        ProductionCovered = $summary.Covered
        Percent = $(if ($summary.Total -gt 0) { [Math]::Round(100.0 * $summary.Covered / $summary.Total, 2) } else { $null })
        ExcludedTestLines = $summary.Excluded
        Exclusions = $ranges
    })
    [void]$mappedFiles.Add($file)
}
if ($rows.Count -eq 0) { throw 'No source files matched the requested root' }
$total = ($rows | Measure-Object -Property ProductionLines -Sum).Sum
$covered = ($rows | Measure-Object -Property ProductionCovered -Sum).Sum
$report = [ordered]@{
    CreatedUtc = [DateTime]::UtcNow.ToString('o')
    Executable = $executablePath
    ExecutableSha256 = (Get-FileHash -LiteralPath $executablePath -Algorithm SHA256).Hash.ToLowerInvariant()
    Profile = $profilePath
    SourceRoot = $sourcePath
    Method = 'Distinct source lines from LLVM LCOV DA records; overlapping LLVM summary mappings are retained separately; only cfg(test) ranges verified against Rust symbols are excluded'
    ProductionLines = $total
    ProductionCovered = $covered
    Percent = [Math]::Round(100.0 * $covered / $total, 2)
    MinimumPercent = $MinimumPercent
    ThresholdPassed = (100.0 * $covered / $total) -ge $MinimumPercent
    Files = $rows.ToArray()
    UnmappedSourceFiles = @(Get-ChildItem -LiteralPath $sourcePath -Recurse -File -Filter '*.rs' |
        Where-Object { -not $mappedFiles.Contains($_.FullName) } |
        ForEach-Object { $_.FullName.Substring($sourcePath.Length).Replace('\', '/') })
    Caveat = 'Coverage is for this instrumented target; generated UI, dependencies, build scripts, other operating systems, and unmapped files are not measured. It does not prove live-client acceptance.'
}
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($output)) | Out-Null
[IO.File]::WriteAllText($output, ($report | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
$rows | Select-Object File,ProductionLines,ProductionCovered,Percent,ExcludedTestLines | Format-Table -AutoSize
Write-Output "Production line coverage: $covered/$total ($($report.Percent)%)"
Write-Output "Report: $output"
if (-not $report.ThresholdPassed) {
    throw "Production coverage $($report.Percent)% is below the required $MinimumPercent%."
}