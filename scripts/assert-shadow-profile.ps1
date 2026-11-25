<#
.SYNOPSIS
  Fails when a recording of the game shows the shadow pass redrawing its static layer at a cost the scene cannot explain.

.DESCRIPTION
  Reads a recording through the editor's MCP server (`profile_gpu`, `profile_frame_times`, `.agents/MCP.md`) and checks, for a recording of the
  packaged game walking, turning and jumping in the Bistro level (`profile_start { target: "game", duration_seconds: 20 }` while the game is driven):

    - `Shadow: static layer` GPU time per serviced frame stays under -StaticLayerP95Ms at the 95th percentile and -StaticLayerMaxMs at its worst
      (measured here: 0.9 ms mean, 1.9 ms worst; a layer redrawn in full every frame on a heavy view measured 27 ms mean, 64 ms worst);
    - the GPU frame stays under -GpuFrameP95Ms at the 95th percentile.

  Exit code 0 when they hold, 1 when one does not (and says which). The thresholds are generous: they catch the order-of-magnitude regression,
  not run-to-run noise (the GPU's clocks move the same recording 30 percent).

.EXAMPLE
  ./scripts/assert-shadow-profile.ps1 -Recording flamegraph_2026-11-25_06-13-21
#>
param(
    [Parameter(Mandatory)][string]$Recording,
    [double]$StaticLayerP95Ms = 5.0,
    [double]$StaticLayerMaxMs = 8.0,
    [double]$GpuFrameP95Ms = 12.0
)

$endpoint = Get-Content "$env:APPDATA\Pulsar\Pulsar_Engine\data\mcp\endpoint.json" -Raw | ConvertFrom-Json
function Call-Tool([string]$Name, [hashtable]$Arguments) {
    $body = @{ jsonrpc = '2.0'; id = 1; method = 'tools/call'; params = @{ name = $Name; arguments = $Arguments } } | ConvertTo-Json -Depth 20 -Compress
    $reply = Invoke-RestMethod $endpoint.url -Method Post -Headers @{ Authorization = "Bearer $($endpoint.token)" } -ContentType 'application/json' -Body $body
    if ($reply.result.structuredContent) { return $reply.result.structuredContent }
    return ($reply.result.content | ForEach-Object { $_.text }) -join "`n" | ConvertFrom-Json
}

$gpu = Call-Tool 'profile_gpu' @{ recording = $Recording }
if (-not $gpu.available) { Write-Error "no GPU timings in $Recording"; exit 1 }
$failures = @()
$static = $gpu.passes | Where-Object { $_.pass -eq 'Shadow: static layer' } | Select-Object -First 1
if ($static) {
    if ($static.per_sample_ms.p95 -gt $StaticLayerP95Ms) { $failures += "Shadow: static layer p95 $($static.per_sample_ms.p95) ms > $StaticLayerP95Ms ms" }
    if ($static.per_sample_ms.max -gt $StaticLayerMaxMs) { $failures += "Shadow: static layer worst $($static.per_sample_ms.max) ms > $StaticLayerMaxMs ms" }
}
if ($gpu.gpu_frame_ms.p95 -gt $GpuFrameP95Ms) { $failures += "GPU frame p95 $($gpu.gpu_frame_ms.p95) ms > $GpuFrameP95Ms ms" }

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Host "FAIL: $_" }
    exit 1
}
Write-Host ("ok: static layer p95 {0} ms, worst {1} ms; GPU frame p95 {2} ms ({3} GPU frames)" -f $static.per_sample_ms.p95, $static.per_sample_ms.max, $gpu.gpu_frame_ms.p95, $gpu.gpu_frames)
exit 0
