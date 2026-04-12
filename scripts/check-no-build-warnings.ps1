$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$previousRustFlags = $env:RUSTFLAGS

try {
    if ([string]::IsNullOrWhiteSpace($previousRustFlags)) {
        $env:RUSTFLAGS = "-Dwarnings"
    } elseif ($previousRustFlags -notmatch '(^|\s)-Dwarnings(\s|$)') {
        $env:RUSTFLAGS = "$previousRustFlags -Dwarnings"
    }

    $output = & cargo build --locked --all-targets --quiet --color never 2>&1 | ForEach-Object {
        $_.ToString()
    }
    $exitCode = $LASTEXITCODE

    foreach ($line in $output) {
        Write-Host $line
    }

    if ($exitCode -ne 0) {
        exit $exitCode
    }

    # Catch warning lines that do not flow through rustc's -Dwarnings path, such
    # as cargo:warning output from build scripts.
    $warningLines = $output | Where-Object {
        $_ -match '^\s*warning(\[[^\]]+\])?:'
    }

    if ($warningLines) {
        Write-Error ("cargo build emitted warning output:`n" + ($warningLines -join "`n"))
    }
} finally {
    if ($null -eq $previousRustFlags) {
        Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
    } else {
        $env:RUSTFLAGS = $previousRustFlags
    }
}
