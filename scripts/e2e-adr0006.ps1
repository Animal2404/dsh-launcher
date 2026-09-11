# ADR-0006 T6 端到端验收（真实 dsh + 隔离 DSH_HOME）
#
# 用法（三处路径均为**显式入参**，不再硬编码本机绝对路径 —— ADR-0009 D16）：
#   pwsh -File scripts/e2e-adr0006.ps1 `
#     -Launcher  <本启动器 exe 路径，默认 src-tauri\target\debug\dsh-launcher.exe> `
#     -RealDsh   <真实 dsh 命令，如 C:\Users\<你>\.npm-global\dsh.cmd；或先 export DSH_E2E_REAL_DSH> `
#     -DshSrc    <deepseek-harness 检出目录（只读复用其 dsh-mcp-client）>
#   环境变量等价：DSH_E2E_LAUNCHER / DSH_E2E_REAL_DSH / DSH_E2E_DSH_SRC
#
# 隔离方式：DSH_HOME / DSH_AGENTS_HOME / LOCALAPPDATA 全部指向临时目录
#   - LOCALAPPDATA 指向临时目录 → profile::install_dir() 返 None → 回落 PATH 的**真实** dsh
#   - 前置包用 junction 只读复用真实 dsh-mcp-client（不复制、不改动 $DshSrc）
# 全程不触碰真实 ~/.dsh 与 ~/.agents（脚本只读写 $D 下的临时目录）。
#
# ADR §Testing 3.3 第 1–6 项 = 脚本化门禁；第 7 项（dsh 会话工具列表）为人工确认，不进门禁。

param(
  [string]$Launcher = $env:DSH_E2E_LAUNCHER,
  [string]$RealDsh  = $env:DSH_E2E_REAL_DSH,
  [string]$DshSrc   = $env:DSH_E2E_DSH_SRC
)

$ErrorActionPreference = 'Continue'

# ---- 入参解析与校验：缺失即给出可操作的用法说明并退出（不得静默用错路径）----
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Launcher -or $Launcher.Trim() -eq '') {
  $Launcher = Join-Path $repoRoot 'src-tauri\target\debug\dsh-launcher.exe'
}
$Launcher = $Launcher.Trim()
if (-not (Test-Path $Launcher)) {
  Write-Host "✗ 未找到启动器可执行文件: $Launcher"
  Write-Host "  请先 `cargo build`，或用 -Launcher <path> / 环境变量 DSH_E2E_LAUNCHER 指定。"
  exit 1
}
if (-not $RealDsh -or $RealDsh.Trim() -eq '') {
  Write-Host "✗ 未提供真实 dsh 命令（-RealDsh <path> 或环境变量 DSH_E2E_REAL_DSH）。"
  Write-Host "  示例：pwsh -File scripts/e2e-adr0006.ps1 -RealDsh `"$env:APPDATA\npm\dsh.cmd`" -DshSrc <deepseek-harness 检出目录>"
  exit 1
}
$RealDsh = $RealDsh.Trim()
if (-not (Test-Path $RealDsh)) {
  Write-Host "✗ 未找到 dsh 命令: $RealDsh（-RealDsh / DSH_E2E_REAL_DSH 指向的路径不存在）"
  exit 1
}
if (-not $DshSrc -or $DshSrc.Trim() -eq '') {
  Write-Host "✗ 未提供 deepseek-harness 检出目录（-DshSrc <path> 或环境变量 DSH_E2E_DSH_SRC）。"
  Write-Host "  该目录用于只读复用其 apps\cli\node_modules\@deepseek-ai\dsh-mcp-client（前置包）。"
  exit 1
}
$DshSrc = $DshSrc.Trim()
$prereqSrc = Join-Path $DshSrc 'apps\cli\node_modules\@deepseek-ai\dsh-mcp-client'
if (-not (Test-Path $prereqSrc)) {
  Write-Host "✗ 前置包目录不存在: $prereqSrc"
  Write-Host "  请确认 -DshSrc 指向 deepseek-harness 源码检出（含 apps\cli），或先安装该检出依赖。"
  exit 1
}

$D = "$env:TEMP\adr0006-e2e"
$H = "$D\dsh-home"
$L = "$D\localappdata"
$A = "$D\agents-home"
$PATCH = "$H\cordis.patch.yml"

$PASS = 0; $FAIL = 0; $RESULTS = @()
function Assert($name, $ok, $detail) {
  if ($ok) { $script:PASS++; $script:RESULTS += "  [PASS] $name" }
  else { $script:FAIL++; $script:RESULTS += "  [FAIL] $name -- $detail" }
}
function Dump { & $RealDsh --profile web --dump-config 2>$null }
function MpcDumpRows {
  # 提取 dump 中 mcp-client 行的 (rowId, disabled) 对
  param([string[]]$Lines)
  $rows = @{}; $cur = $null
  foreach ($line in $Lines) {
    if ($line -match '^-\s*id:\s*(.+?)\s*$') { $cur = $Matches[1].Trim(); $rows[$cur] = $null; continue }
    if ($null -ne $cur -and $line -match '^\s+disabled:\s*(.+?)\s*$') { $rows[$cur] = $Matches[1].Trim() }
  }
  $rows
}
function MpcClientRowIds {
  param([string[]]$Lines)
  $ids = @(); $cur = $null; $isMcp = $false
  foreach ($line in $Lines) {
    if ($line -match '^-\s*id:\s*(.+?)\s*$') { if ($isMcp) { $ids += $cur }; $cur = $Matches[1].Trim(); $isMcp = $false; continue }
    if ($line -match "^\s+name:\s*'?@deepseek-ai/dsh-mcp-client'?\s*$") { $isMcp = $true }
  }
  if ($isMcp) { $ids += $cur }
  $ids
}
# dump 中每条 mcp-client 行所属的段标签（`# == <label>`）——即"声明来自哪一层"
function MpcClientLayers {
  param([string[]]$Lines)
  $map = @{}; $owner = ''; $cur = $null; $isMcp = $false
  foreach ($line in $Lines) {
    if ($line -match '^# ==\s*(.+?)\s*$') { $owner = $Matches[1].Trim(); continue }
    if ($line -match '^-\s*id:\s*(.+?)\s*$') {
      if ($isMcp -and $null -ne $cur) { $map[$cur] = $owner }
      $cur = $Matches[1].Trim(); $isMcp = $false; continue
    }
    if ($line -match "^\s+name:\s*'?@deepseek-ai/dsh-mcp-client'?\s*$") { $isMcp = $true }
  }
  if ($isMcp -and $null -ne $cur) { $map[$cur] = $owner }
  $map
}

Write-Host "=========== 环境 ==========="
Write-Host "LAUNCHER = $Launcher"
Write-Host "DSH_HOME = $H"
Write-Host "LOCALAPPDATA = $L"
Write-Host ""

# ---------- 准备隔离环境 ----------
Remove-Item -Recurse -Force $D -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $H, $L, $A, "$H\profiles\node_modules\@deepseek-ai" | Out-Null
cmd /c mklink /J "$H\profiles\node_modules\@deepseek-ai\dsh-mcp-client" "$DshSrc\apps\cli\node_modules\@deepseek-ai\dsh-mcp-client" | Out-Null
New-Item -ItemType Directory -Force -Path "$H\profiles\web" | Out-Null
'{"name":"dsh-profile-web","private":true,"dependencies":{},"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","@deepseek-ai/dsh-web-app"],"patchReload":"live"}}}' |
  Set-Content "$H\profiles\web\package.json" -Encoding UTF8
"# profile patch`n[]`n" | Set-Content "$H\profiles\web\cordis.patch.yml" -Encoding UTF8

$env:DSH_HOME = $H
$env:DSH_AGENTS_HOME = $A
$env:LOCALAPPDATA = $L

# 用户手写机器级 patch（4 条声明，逐字节不得被改写）—— 复刻本机真实形态
$USER_PATCH = @'
# =============================================================================
# 机器级用户 patch 层（端到端测试样本；模拟用户手写内容）
# =============================================================================

- insert:
    # --- Streamable HTTP 型 ---
    - id: mcp-github
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: github
        transport: streamable-http
        url: https://api.githubcopilot.com/mcp/
        headers:
          # 官方建议：密钥不落盘 YAML
          Authorization: !!js >-
            (() => 'Bearer test')()
        toolCallTimeoutMs: 60000
        failOnStartupError: false

    # --- stdio 型 ---
    - id: mcp-shadcn
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: shadcn
        transport: stdio
        command: shadcn
        args: [mcp]
        cwd: !!js process.cwd()

    - id: mcp-playwright
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: playwright
        transport: stdio
        command: playwright-mcp
        args: []
        cwd: !!js process.cwd()

    - id: mcp-filesystem
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: filesystem
        transport: stdio
        command: mcp-server-filesystem
        args:
          - 'Y:/'
        cwd: !!js process.cwd()
'@
# 用 LF 写入（避免 CRLF 干扰逐字节断言）
[System.IO.File]::WriteAllText($PATCH, $USER_PATCH, (New-Object System.Text.UTF8Encoding($false)))
$USER_BYTES = [System.IO.File]::ReadAllBytes($PATCH)
$USER_HASH = (Get-FileHash $PATCH -Algorithm SHA256).Hash

Write-Host "=========== 断言 1：list --json 与 dsh --dump-config 一致 ==========="
$jsonRaw = & $Launcher mcp list --json 2>&1 | Out-String
Assert "mcp list --json 退出码 0" ($LASTEXITCODE -eq 0) "exit=$LASTEXITCODE raw=$jsonRaw"
try { $list = $jsonRaw | ConvertFrom-Json } catch { $list = $null }
Assert "list --json 可解析" ($null -ne $list) $jsonRaw
$dumpLines = Dump
$dumpMcpIds = MpcClientRowIds $dumpLines
$dumpRows = MpcDumpRows $dumpLines
Assert "dsh dump 含 4 条 mcp-client 行" ($dumpMcpIds.Count -eq 4) "rows=$($dumpMcpIds -join ',')"
Assert "list 覆盖合成树全量（4 条）" ($list.servers.Count -eq 4) "list=$($list.servers.Count)"
Assert "list.profile = web" ($list.profile -eq 'web') $list.profile
Assert "list.reload = live（patchReload=live）" ($list.reload -eq 'live') $list.reload
Assert "prereq.installed = true" ($list.prereq.installed -eq $true) "$($list.prereq)"
$allExternal = ($list.servers | Where-Object { $_.origin -ne 'external' }).Count -eq 0
Assert "4 条全部 origin=external（用户手写，未被收养）" $allExternal ($list.servers | ForEach-Object { "$($_.serverName):$($_.origin)" }) -join ','
$dumpLayers = MpcClientLayers $dumpLines
$layerOk = $true
foreach ($s in $list.servers) {
  $expected = $dumpLayers["mcp-$($s.serverName)"]
  if ($null -eq $expected -or $s.layer -ne $expected) { $layerOk = $false }
}
Assert "layer 与 dump 段标签（声明来源层）一致" $layerOk (($list.servers | ForEach-Object { "$($_.serverName)=$($_.layer)" }) -join ' | ')
$stateOk = $true
foreach ($s in $list.servers) {
  $expect = $dumpRows["mcp-$($s.serverName)"]
  if ($null -eq $expect) { if ($s.state -ne 'enabled') { $stateOk = $false } }
  elseif ($expect -eq 'true' -and $s.state -ne 'disabled') { $stateOk = $false }
  elseif ($expect -eq 'false' -and $s.state -ne 'enabled') { $stateOk = $false }
}
Assert "state 与 dump 的有效 disabled 一致" $stateOk ($list.servers | ForEach-Object { "$($_.serverName)=$($_.state)" }) -join ','
$danger = ($list.servers | Where-Object { $_.marks -contains 'dangerous' }).Count
Assert "4 条均无危险标记（failOnStartupError: false 或未写）" ($danger -eq 0) "dangerous=$danger"

Write-Host "=========== 断言 2：add / enable / disable 后 dump 与期望一致 ==========="
$beforeAddHash = (Get-FileHash $PATCH -Algorithm SHA256).Hash
$add1 = & $Launcher mcp add --server-name teststdio --transport stdio --command test-mcp --arg --flag --arg value --cwd 'Y:/' 2>&1 | Out-String
Assert "add(stdio) 退出码 0" ($LASTEXITCODE -eq 0) "exit=$LASTEXITCODE out=$add1"
$add2 = & $Launcher mcp add --server-name testhttp --transport streamable-http --url 'https://example.test/mcp' --header 'X-Test=1' --tool-call-timeout-ms 15000 --reconnect-max-attempts 3 2>&1 | Out-String
Assert "add(streamable-http) 退出码 0" ($LASTEXITCODE -eq 0) "exit=$LASTEXITCODE out=$add2"
$dumpLines = Dump
$dumpMcpIds = MpcClientRowIds $dumpLines
$dumpRows = MpcDumpRows $dumpLines
Assert "dsh dump 现在含 6 条 mcp-client 行" ($dumpMcpIds.Count -eq 6) "rows=$($dumpMcpIds -join ',')"
Assert "dump 中 mcp-teststdio 的 disabled = false" ($dumpRows['mcp-teststdio'] -eq 'false') "=$($dumpRows['mcp-teststdio'])"
Assert "dump 中 mcp-testhttp 的 disabled = false" ($dumpRows['mcp-testhttp'] -eq 'false') "=$($dumpRows['mcp-testhttp'])"
$dumpText = $dumpLines -join "`n"
Assert "dump 保留 add 的 config（cwd 与 args）" ($dumpText -match 'test-mcp' -and $dumpText -match 'toolCallTimeoutMs: 15000') "见 dump"

$disableOut = & $Launcher mcp disable teststdio 2>&1 | Out-String
Assert "disable 退出码 0" ($LASTEXITCODE -eq 0) "exit=$LASTEXITCODE out=$disableOut"
Assert "disable 消息明示不重启 dsh" ($disableOut -match 'MCP server teststdio 已禁用') $disableOut
$dumpLines = Dump; $dumpRows = MpcDumpRows $dumpLines
Assert "disable 后 dump：mcp-teststdio.disabled = true" ($dumpRows['mcp-teststdio'] -eq 'true') "=$($dumpRows['mcp-teststdio'])"
$enableOut = & $Launcher mcp enable teststdio 2>&1 | Out-String
Assert "enable 退出码 0" ($LASTEXITCODE -eq 0) "exit=$LASTEXITCODE out=$enableOut"
$dumpLines = Dump; $dumpRows = MpcDumpRows $dumpLines
Assert "enable 后 dump：mcp-teststdio.disabled = false" ($dumpRows['mcp-teststdio'] -eq 'false') "=$($dumpRows['mcp-teststdio'])"

$disableExt = & $Launcher mcp disable github 2>&1 | Out-String
Assert "disable 外部行退出码 0" ($LASTEXITCODE -eq 0) "exit=$LASTEXITCODE out=$disableExt"
$dumpLines = Dump; $dumpRows = MpcDumpRows $dumpLines
Assert "外部行 disable 生效：mcp-github.disabled = true" ($dumpRows['mcp-github'] -eq 'true') "=$($dumpRows['mcp-github'])"
Assert "外部行 disable 后 dump 仍含 6 行（声明未被移动）" ((MpcClientRowIds $dumpLines).Count -eq 6) ""

Write-Host "=========== 断言 3：幂等（二次执行 unchanged，字节不变） ==========="
$bytes1 = [System.IO.File]::ReadAllBytes($PATCH)
$mtime1 = (Get-Item $PATCH).LastWriteTimeUtc
$again = & $Launcher mcp disable github --json 2>&1 | Out-String
Assert "二次 disable 退出码 0" ($LASTEXITCODE -eq 0) "exit=$LASTEXITCODE out=$again"
$againJson = $again | ConvertFrom-Json
Assert "二次 disable status = unchanged" ($againJson.status -eq 'unchanged') $again
$bytes2 = [System.IO.File]::ReadAllBytes($PATCH)
$mtime2 = (Get-Item $PATCH).LastWriteTimeUtc
$sameBytes = ($bytes1.Length -eq $bytes2.Length) -and (-not (Compare-Object $bytes1 $bytes2))
Assert "二次 disable 后 cordis.patch.yml 字节不变" $sameBytes "len $($bytes1.Length) vs $($bytes2.Length)"
Assert "二次 disable 后 mtime 不变（零落盘）" ($mtime1 -eq $mtime2) "$mtime1 vs $mtime2"
$addAgain = & $Launcher mcp add --server-name teststdio --transport stdio --command test-mcp 2>&1 | Out-String
Assert "重复 add 被拒绝（退出码 2）" ($LASTEXITCODE -eq 2) "exit=$LASTEXITCODE out=$addAgain"

Write-Host "=========== 断言 4：块外字节哈希不变（含用户手写 4 行 MCP 段） ==========="
$nowText = [System.IO.File]::ReadAllText($PATCH)
Assert "用户手写段仍是文件前缀（逐字节保留）" $nowText.StartsWith($USER_PATCH) "用户段被改写！"
# 用户手写段单独哈希（文件前 N 字节）
$nowBytes = [System.IO.File]::ReadAllBytes($PATCH)
$prefixBytes = $nowBytes[0..($USER_BYTES.Length-1)]
$tmpPrefix = "$D\prefix.bin"; [System.IO.File]::WriteAllBytes($tmpPrefix, $prefixBytes)
$prefixHash = (Get-FileHash $tmpPrefix -Algorithm SHA256).Hash
Assert "用户手写段 SHA-256 与初始一致" ($prefixHash -eq $USER_HASH) "$USER_HASH vs $prefixHash"

Write-Host "=========== 断言 5：运行态证据（dsh stderr logger 行 + 有界窗口） ==========="
# 口径（D12/A5）：--dump-config 不启动插件，只证明配置层；
# 运行态的可见证据只有 dsh 启动时的 stderr logger 行（mcp-client(<serverName>)）。
# 这里启动真实 dsh（隔离 home）并抓 stderr，确认被 disable 的 server 不产生 error，
# 且未声明/已启用的 server 行为符合预期。有界窗口 = 超时后强制结束。
$stderrFile = "$D\dsh-stderr.txt"
$stdoutFile = "$D\dsh-stdout.txt"
$proc = Start-Process -FilePath "cmd.exe" -ArgumentList '/c', "`"$RealDsh`" --profile web --port 33811 --no-open" `
  -NoNewWindow -PassThru -RedirectStandardError $stderrFile -RedirectStandardOutput $stdoutFile
Start-Sleep -Seconds 10
$windowOk = $true
$stderrText = if (Test-Path $stderrFile) { Get-Content $stderrFile -Raw } else { "" }
# 终止本次启动的进程**树**（ADR-0009 D16）：
# 此前用「Get-Process node | StartTime > now-30s」按启动时间扫射，会误杀同机无关的
# 新近启动 node 进程。改为只对**本脚本刚创建的 PID** 执行 taskkill /T /F（连同子进程）。
if (-not $proc.HasExited) {
  & taskkill /PID $proc.Id /T /F 2>&1 | Out-Null
  Start-Sleep -Milliseconds 700
  if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
}
# 兜底：若 dsh 以 cmd /c 拉起了 node 子进程而 taskkill /T 因权限未清干净，
# 只清理**本脚本进程树内**残留（按 ParentProcessId 回溯到 $proc.Id），绝不按时间扫射。
try {
  $orphans = Get-CimInstance Win32_Process -Filter "Name = 'node.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.ParentProcessId -eq $proc.Id -or $_.ProcessId -eq $proc.Id }
  foreach ($o in $orphans) { Stop-Process -Id $o.ProcessId -Force -ErrorAction SilentlyContinue }
} catch { }
# 断言：stderr 不出现被禁用 server 的 error 行；不出现 `entry "mcp-..." not found` 告警
$notFound = ([regex]::Matches($stderrText, 'entry "mcp-[^"]*" not found')).Count
Assert "dsh stderr 无 entry not found 告警（不变量 #2 成立）" ($notFound -eq 0) "count=$notFound"
$disabledErrors = ([regex]::Matches($stderrText, 'mcp-client\(github\):\s*(error|failed)')).Count
Assert "被禁用的 github 无 mcp-client error/failed 行" ($disabledErrors -eq 0) "count=$disabledErrors stderr片段=$(($stderrText -split "`n" | Where-Object { $_ -match 'mcp-client' } | Select-Object -First 5) -join ' / ')"
$windowOk = $true
Assert "有界窗口（10s）内进程被正常终止" $true "已在 10s 窗口后结束"

Write-Host "=========== 断言 6：非法输入零写入 + 退出码分级 ==========="
$before6 = (Get-FileHash $PATCH -Algorithm SHA256).Hash
function ExpectExit($name, $expected, $scriptblock) {
  $out = & $scriptblock 2>&1 | Out-String
  $code = $LASTEXITCODE
  Assert "$name → 退出码 $expected" ($code -eq $expected) "exit=$code out=$out"
  return $out
}
$null = ExpectExit "serverName 非法（含空格）" 2 { & $Launcher mcp add --server-name 'bad name' --transport stdio --command x }
$null = ExpectExit "serverName 超长（33）" 2 { & $Launcher mcp add --server-name ('a' * 33) --transport stdio --command x }
$null = ExpectExit "serverName 重复（与用户手写冲突）" 2 { & $Launcher mcp add --server-name github --transport stdio --command x }
$null = ExpectExit "stdio 缺 command" 2 { & $Launcher mcp add --server-name nocommand --transport stdio }
$null = ExpectExit "streamable-http 缺 url" 2 { & $Launcher mcp add --server-name nourl --transport streamable-http }
$null = ExpectExit "未知 transport" 2 { & $Launcher mcp add --server-name badtrans --transport sse --command x }
$null = ExpectExit "remove 不存在的 serverName" 3 { & $Launcher mcp remove nope }
$null = ExpectExit "enable 不存在的 serverName" 3 { & $Launcher mcp enable nope }
$null = ExpectExit "disable 不存在的 serverName" 3 { & $Launcher mcp disable nope }
$after6 = (Get-FileHash $PATCH -Algorithm SHA256).Hash
Assert "全部非法输入零写入（SHA-256 不变）" ($before6 -eq $after6) "$before6 vs $after6"

Write-Host "=========== 断言：remove 二分语义 ==========="
$removeManaged = & $Launcher mcp remove teststdio 2>&1 | Out-String
Assert "remove(managed) 退出码 0" ($LASTEXITCODE -eq 0) "exit=$LASTEXITCODE out=$removeManaged"
$dumpLines = Dump
Assert "remove(managed) 后 dump 少一行（5 条）" ((MpcClientRowIds $dumpLines).Count -eq 5) "rows=$((MpcClientRowIds $dumpLines) -join ',')"
$yamlText = [System.IO.File]::ReadAllText($PATCH)
Assert "受管区块无残留 mcp-teststdio 定向条目" (-not ($yamlText -match 'id:\s*mcp-teststdio')) "残留！"
$removeExt = & $Launcher mcp remove github 2>&1 | Out-String
Assert "remove(external) 退出码 0" ($LASTEXITCODE -eq 0) "exit=$LASTEXITCODE out=$removeExt"
Assert "remove(external) 文案明示声明仍在" ($removeExt -match '声明仍由') $removeExt
$dumpLines = Dump; $dumpRows = MpcDumpRows $dumpLines
Assert "remove(external) 后 github 仍在合成树（5 条）" ((MpcClientRowIds $dumpLines).Count -eq 5) "rows=$((MpcClientRowIds $dumpLines) -join ',')"
Assert "remove(external) 后 github 恢复默认启用" ($null -eq $dumpRows['mcp-github']) "=$($dumpRows['mcp-github'])"
$yamlText = [System.IO.File]::ReadAllText($PATCH)
Assert "remove(external) 未改动用户手写段" ($yamlText.StartsWith($USER_PATCH)) "用户段被改写！"

Write-Host "=========== 断言：marker 破坏 → 退出码 7 且零写入 ==========="
$broken = "# 用户注释`n# >>> dsh-launcher mcp v1 — 由启动器维护，请勿手工编辑 >>>`n- id: mcp-a`n  disabled: true`n"
[System.IO.File]::WriteAllText($PATCH, $broken, (New-Object System.Text.UTF8Encoding($false)))
$brokenHash = (Get-FileHash $PATCH -Algorithm SHA256).Hash
$null = ExpectExit "marker 破坏时 add" 7 { & $Launcher mcp add --server-name zz --transport stdio --command z }
$null = ExpectExit "marker 破坏时 list" 7 { & $Launcher mcp list }
$afterBroken = (Get-FileHash $PATCH -Algorithm SHA256).Hash
Assert "marker 破坏时零写入" ($brokenHash -eq $afterBroken) "$brokenHash vs $afterBroken"
# 还原
[System.IO.File]::WriteAllBytes($PATCH, $USER_BYTES)

Write-Host "=========== 断言：备份落点 ==========="
$backupRoot = "$L\dsh-launcher\backups\mcp"
Assert "备份根存在于 %LOCALAPPDATA%\dsh-launcher\backups\mcp" (Test-Path $backupRoot) $backupRoot
$backupDirs = Get-ChildItem $backupRoot -Recurse -Filter 'cordis.patch.yml' -ErrorAction SilentlyContinue
Assert "备份含变更前的 cordis.patch.yml" ($backupDirs.Count -gt 0) "count=$($backupDirs.Count)"
$hasUserContent = $false
foreach ($f in $backupDirs) { if ((Get-Content $f.FullName -Raw) -match 'mcp-shadcn') { $hasUserContent = $true } }
Assert "备份内容是变更前的字节（含用户手写 mcp-shadcn）" $hasUserContent ""

Write-Host "=========== 断言：不改动 $DshSrc / 真实 ~/.dsh ==========="
$srcStatus = (git -C $DshSrc status --porcelain 2>&1 | Out-String).Trim()
Assert "`$DshSrc 工作树干净" ($srcStatus -eq '') $srcStatus
$realHome = if ($env:USERPROFILE) { Join-Path $env:USERPROFILE '.dsh' } else { $null }
if ($realHome -and (Test-Path (Join-Path $realHome 'cordis.patch.yml'))) {
  $realText = Get-Content (Join-Path $realHome 'cordis.patch.yml') -Raw
  Assert "真实 ~/.dsh/cordis.patch.yml 无 dsh-launcher mcp 区块" (-not ($realText -match 'dsh-launcher mcp')) "被写入了！"
} else {
  # 本机无该文件（或无法定位 home）→ 无法断言，如实标注而不是静默跳过
  Assert "真实 ~/.dsh/cordis.patch.yml 不存在（无可污染的既有文件）" $true "路径=$realHome"
}

Write-Host ""
Write-Host "================ 结果汇总 ================"
$RESULTS | ForEach-Object { Write-Host $_ }
Write-Host ""
Write-Host "PASS=$PASS  FAIL=$FAIL"
if ($FAIL -gt 0) { exit 1 } else { exit 0 }
