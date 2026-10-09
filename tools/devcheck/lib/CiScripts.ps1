# CI 侧的不变量（被 devcheck.ps1 dot-source）
#
# 这里断言的都是「本地跑得好好的、到了 CI 或交付环节才炸」的东西：工作流用的
# action 版本、每一层有没有真的接进工作流、构建入口与交付名是否一致。

function Test-CiScripts {
    $notes = [System.Collections.Generic.List[string]]::new()

    # 1) 工作流用的 action 版本必须 ≥ tools/devcheck/README.md 登记的下限
    #    （低于下限会在 runner 上打 Node 弃用告警）。README 改了、工作流忘了跟着升，
    #    或者反过来把工作流降级，这里都会失败。
    $readme = Get-Content -LiteralPath (Join-Path $DevCheckRoot 'README.md') -Raw
    # 取第一行「既能认出是版本下限、又真的解析出 action」的文本：README 里出现这个
    # 说法的地方不止一处（标题、说明），按内容挑才不会挑到标题。
    $floors = @{}
    foreach ($row in @(($readme -split "`n") | Where-Object { $_ -match 'action 的版本下限' })) {
        foreach ($m in [regex]::Matches($row, '`([\w\-]+/[\w\-]+)`\s*≥\s*v(\d+)')) {
            $floors[$m.Groups[1].Value] = [int]$m.Groups[2].Value
        }
        if ($floors.Count) { break }
    }
    if ($floors.Count -eq 0) { throw 'tools/devcheck/README.md 里找不到解析得出 action 的「版本下限」那一行' }

    $checked = 0
    foreach ($wf in Get-ChildItem -LiteralPath (Join-Path $RepoRoot '.github/workflows') -Filter '*.yml' -File) {
        $text = Get-Content -LiteralPath $wf.FullName -Raw
        foreach ($m in [regex]::Matches($text, 'uses:\s*([\w\-]+/[\w\-]+)@v(\d+)')) {
            $name = $m.Groups[1].Value
            if (-not $floors.ContainsKey($name)) { continue }
            $used = [int]$m.Groups[2].Value
            $checked++
            if ($used -lt $floors[$name]) {
                throw "$($wf.Name) 里 $name@v$used 低于 README 登记的下限 v$($floors[$name])"
            }
        }
    }
    if ($checked -eq 0) { throw '工作流里没有找到任何受版本下限约束的 action，检查正则是否失效' }
    $notes.Add("$($checked) 处 action 引用不低于下限")

    # 2) all 集合里的每一层都必须在 devcheck.yml 上真跑过 —— 新加一层却忘了接进
    #    工作流，本地和 CI 都会「绿」，那层等于没写。
    $main = Get-Content -LiteralPath (Join-Path $DevCheckRoot 'devcheck.ps1') -Raw
    $allBlock = [regex]::Match($main, "(?m)^\`$allLayers\s*=\s*@\(([^)]*)\)")
    if (-not $allBlock.Success) { throw 'devcheck.ps1 里找不到 $allLayers 的定义，解析规则失效了' }
    $layers = @([regex]::Matches($allBlock.Groups[1].Value, "'([a-z0-9]+)'") |
        ForEach-Object { $_.Groups[1].Value })
    if ($layers.Count -lt 5) { throw "只从 all 集合里解析出 $($layers.Count) 层，解析规则失效了" }

    $workflow = Get-Content -LiteralPath (Join-Path $RepoRoot '.github/workflows/devcheck.yml') -Raw
    # 无参数的那一步才会跑 $wanted 里由 all 定义的层；只写 -Layer all 不会命中这条。
    $runsAll = $workflow -match '(?m)^\s*(?:-\s*)?run:\s*pwsh\s+-NoProfile\s+-File\s+\S*devcheck\.ps1\s*$'
    if (-not $runsAll) {
        throw 'devcheck.yml 里没有一步是不带 -Layer 跑 devcheck.ps1 —— all 集合的层不会在 CI 上执行'
    }
    if ($workflow -notmatch '-SelfTest') {
        throw 'devcheck.yml 没有跑 -SelfTest —— 自检不在 CI 上执行，等于没有证明检查会失败'
    }
    $single = @($layers | Where-Object { $workflow -match "-Layer\s+$_\b" })
    $notes.Add("devcheck.yml 覆盖 $($layers.Count) 层（$($single.Count) 层有单独步骤：$($single -join '、')）")

    # 3) 构建入口与交付名一致：三处都必须是「拼出 bundle → 断言它比 cargo 裸 builder
    #    大 → 以 kirara-builder.exe 交付」。少任何一处，交付物就可能悄悄变回裸 builder。
    $entries = @(
        @{ File = 'build.ps1'; Text = [System.IO.File]::ReadAllText((Join-Path $RepoRoot 'build.ps1')) }
        @{ File = '.github/workflows/build.yml'; Text = [System.IO.File]::ReadAllText((Join-Path $RepoRoot '.github/workflows/build.yml')) }
        @{ File = '.github/workflows/p6-e2e.yml'; Text = [System.IO.File]::ReadAllText((Join-Path $RepoRoot '.github/workflows/p6-e2e.yml')) }
    )
    foreach ($e in $entries) {
        if ($e.Text -notmatch 'kachina-builder-bundle\.exe') {
            throw "$($e.File) 没有引用拼接体 kachina-builder-bundle.exe —— 交付物可能是 cargo 的裸 builder"
        }
        if ($e.Text -notmatch [regex]::Escape('kirara-builder.exe')) {
            throw "$($e.File) 没有以 kirara-builder.exe 交付 —— 下游按固定文件名取用"
        }
        if ($e.Text -notmatch 'is not larger than cargo builder|不大于 cargo builder') {
            throw "$($e.File) 没有断言拼接体大于 cargo 裸 builder —— installer 没拼进去也能过"
        }
    }
    $notes.Add('build.ps1 / build.yml / p6-e2e.yml 的拼接与交付名一致')

    # 4) package.json 的 build 脚本是上面三处共同的实现：三步缺一不可
    #    （前端产物 → 原生 release 产物 → 拼接）。
    $pkg = Get-Content -LiteralPath (Join-Path $RepoRoot 'package.json') -Raw | ConvertFrom-Json
    $buildScript = $pkg.PSObject.Properties['scripts'].Value.PSObject.Properties['build'].Value
    foreach ($step in @('rsbuild build', 'cargo build --release --target x86_64-pc-windows-msvc', 'merge-release-bundle.mjs')) {
        if ($buildScript -notmatch [regex]::Escape($step)) {
            throw "package.json 的 build 脚本缺少「$step」"
        }
    }
    $notes.Add('package.json 的 build 脚本三步齐全')

    return ($notes -join '；')
}
