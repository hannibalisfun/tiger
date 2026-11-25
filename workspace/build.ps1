param([string]$ToolchainDirectory)

$ErrorActionPreference = 'Stop'
$repo = (Get-Item -LiteralPath $PSScriptRoot).Parent
$cargo = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
$start = [Diagnostics.ProcessStartInfo]::new()
$start.WorkingDirectory = $repo.FullName
$start.UseShellExecute = $false
$start.Arguments = 'build --locked --release -p ck3-tiger --bin ck3-tiger'

if ($cargo -and !$ToolchainDirectory) {
    $start.FileName = $cargo.Source
} else {
    if (!$ToolchainDirectory) {
        $ToolchainDirectory = Join-Path $repo.Parent.FullName 'tmp/Tiger-CK3-build-tools'
    }
    $tools = (Resolve-Path -LiteralPath $ToolchainDirectory).Path
    $rust = Join-Path $tools 'rust/bin'
    $gnu = Join-Path $tools 'mingw64/bin'
    $linker = Join-Path $tools 'rust/lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained'
    $llvm = Join-Path $tools 'llvm-mingw-20261117-msvcrt-x86_64/bin'
    $gitRoot = (Get-Item -LiteralPath (Get-Command git.exe).Source).Directory.Parent.FullName
    $gitDlls = Join-Path $gitRoot 'mingw64/bin'
    $start.FileName = Join-Path $rust 'cargo.exe'
    $start.Arguments += ' --offline'
    $envs = $start.EnvironmentVariables
    $envs['PATH'] = "$rust;$linker;$gnu;$gitDlls;$llvm;$env:PATH"
    $envs['CARGO_HOME'] = Join-Path $tools 'cargo-cache'
    $envs['CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER'] = Join-Path $linker 'x86_64-w64-mingw32-gcc.exe'
    $envs['RUSTFLAGS'] = '-C link-self-contained=yes -C debuginfo=0'
    $envs['CC_x86_64_pc_windows_gnu'] = Join-Path $llvm 'x86_64-w64-mingw32-clang.exe'
    $envs['AR_x86_64_pc_windows_gnu'] = Join-Path $gnu 'ar.exe'
}

$process = [Diagnostics.Process]::Start($start)
$process.WaitForExit()
exit $process.ExitCode
