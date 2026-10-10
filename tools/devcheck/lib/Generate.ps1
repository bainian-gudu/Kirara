# 生成 tools/devcheck/rust/logic/src/gen 下的检查用源码（被 devcheck.ps1 dot-source）
#
# 生成物一律不入库（见 .gitignore 的 tools/devcheck/rust/logic/src/gen/）。
#
# 抽取的是「安全阀与纯逻辑」那批 item：它们决定删什么、放行什么路径，必须能在任意
# 平台被断言；Windows 专有的 IO / 注册表 / 计划任务实现不在抽取范围内（见 README
# 的覆盖范围一节）。

Set-StrictMode -Version Latest

# 每个清单里的 item 都必须在源文件里找得到，否则 Get-RustItem 会抛错 ——
# 上游重命名/删除时 devcheck 立刻失败，而不是静默少测。

# 路径越界判定（安装清单、卸载清单、包内路径共用）。
$script:LogicStagingItems = @(
    @{ Kind = 'fn'; Name = 'is_safe_rel' }
    @{ Kind = 'fn'; Name = 'try_join_rel' }
)

# 卸载侧安全阀：删目录、删快捷方式、删计划任务、环境变量展开、跨用户重放、%TEMP% 白名单。
$script:LogicUninstallItems = @(
    @{ Kind = 'fn'; Name = 'is_under_system_root' }
    @{ Kind = 'fn'; Name = 'path_eq' }
    @{ Kind = 'fn'; Name = 'is_protected_root' }
    @{ Kind = 'fn'; Name = 'is_safe_delete_root' }
    @{ Kind = 'fn'; Name = 'is_safe_shortcut_path' }
    @{ Kind = 'fn'; Name = 'is_safe_task_name' }
    @{ Kind = 'fn'; Name = 'key_belongs_to_product' }
    @{ Kind = 'fn'; Name = 'value_belongs_to_product' }
    @{ Kind = 'const'; Name = 'PER_USER_CLEANUP_ROOTS' }
    @{ Kind = 'const'; Name = 'PER_USER_DENY_LEAVES' }
    @{ Kind = 'fn'; Name = 'expand_env_vars' }
    @{ Kind = 'fn'; Name = 'expand_path_list' }
    @{ Kind = 'fn'; Name = 'profile_relative_tail' }
    @{ Kind = 'fn'; Name = 'is_installer_temp_artifact' }
)

# 安装计划里的路径归一化与模板展开：清单成员、`${INSTALL_PATH}` 替换、
# 「文件在不在安装目录下」这些判定直接决定装到哪、删什么。
$script:LogicPlanItems = @(
    @{ Kind = 'fn'; Name = 'normalize_rel' }
    @{ Kind = 'fn'; Name = 'is_safe_member' }
    @{ Kind = 'fn'; Name = 'normalize_full' }
    @{ Kind = 'fn'; Name = 'expand_template' }
    @{ Kind = 'fn'; Name = 'join_install' }
    @{ Kind = 'fn'; Name = 'is_under' }
    @{ Kind = 'fn'; Name = 'strip_install_prefix' }
)

# 打包器：协议内联、图片 / 图标取用与未识别配置键点名（整个 pack.rs 依赖 builder
# 的一堆模块，只抽这几个纯函数）。
$script:LogicPackItems = @(
    @{ Kind = 'const'; Name = 'PACK_ONLY_KEYS' }
    @{ Kind = 'fn'; Name = 'unknown_config_keys' }
    @{ Kind = 'fn'; Name = 'config_relative_path' }
    @{ Kind = 'fn'; Name = 'take_pack_file' }
    @{ Kind = 'fn'; Name = 'resolve_agreement' }
)

# 配置键清单：与 ProjectConfig 的 serde 字段一致（漂移由 kachina 本体的单测钉住），
# 这里断言迁移过来的那几组键都还在、上游已移除的键不在。
$script:LogicConfigKeysItems = @(
    @{ Kind = 'const'; Name = 'PROJECT_CONFIG_KEYS' }
)

# 下载后执行的验签结论判定：状态与 Subject 逐段精确比对，子串匹配会把冒名写法放进来。
$script:LogicSecureTempItems = @(
    @{ Kind = 'fn'; Name = 'is_trusted_microsoft_signature' }
)

# MirrorChyan 包内条目名解码：替代 zip fork「不看标志位、按 UTF-8 解」的那条语义，
# 解码错了中文文件名会变乱码，路径安全判定也跟着错。
$script:LogicMirrorcItems = @(
    @{ Kind = 'fn'; Name = 'decode_entry_name' }
)

# 包体识别（PE 起点）与嵌入名规则：识别错了会拿安装器当 builder 打包，嵌入名两边
# 不一致会静默丢数据。
$script:LogicBuilderItems = @(
    @{ Kind = 'const'; Name = 'PE_LFANEW_MIN' }
    @{ Kind = 'const'; Name = 'PE_LFANEW_MAX' }
    @{ Kind = 'fn'; Name = 'preferred_file_hash' }
    @{ Kind = 'fn'; Name = 'pe_image_starts' }
    @{ Kind = 'fn'; Name = 'is_pe_at' }
)

# 嵌入名长度与保留名规则（TLV / 索引名）。
$script:LogicEmbeddedItems = @(
    @{ Kind = 'const'; Name = 'TLV_NAME_MAX' }
    @{ Kind = 'const'; Name = 'INDEX_NAME_MAX' }
    @{ Kind = 'const'; Name = 'INTERNAL_NAMES' }
    @{ Kind = 'enum'; Name = 'EmbeddedNameError' }
    @{ Kind = 'fn'; Name = 'is_internal_name' }
    @{ Kind = 'fn'; Name = 'is_embedded_name' }
    @{ Kind = 'fn'; Name = 'check_len' }
    @{ Kind = 'fn'; Name = 'invalid' }
    @{ Kind = 'fn'; Name = 'is_forbidden_char' }
    @{ Kind = 'fn'; Name = 'is_reserved_device' }
    @{ Kind = 'fn'; Name = 'is_com_or_lpt' }
    @{ Kind = 'fn'; Name = 'check_tlv_name' }
    @{ Kind = 'fn'; Name = 'check_index_name' }
    @{ Kind = 'fn'; Name = 'check_embedded_name' }
)

# 解包路径安全阀：包内相对路径必须落在输出根内。
$script:LogicExtractItems = @(
    @{ Kind = 'fn'; Name = 'relative_under_root' }
    @{ Kind = 'fn'; Name = 'sanitize_output_name' }
)

# H3 证书固定（pinning）：固定值比对错了等于形同虚设，而 H3 传输层只在 Windows 上
# 能真连，本地只能靠这些断言兜住。SPKI 的 DER 定位必须与 openssl 的结果逐字节一致。
$script:LogicH3Items = @(
    @{ Kind = 'enum';   Name = 'PinningMode' }
    @{ Kind = 'enum';   Name = 'PinTarget' }
    @{ Kind = 'struct'; Name = 'PinConfig' }
    @{ Kind = 'fn';     Name = 'sha256' }
    @{ Kind = 'fn';     Name = 'extract_spki_der' }
    @{ Kind = 'fn';     Name = 'compute_spki_hash' }
    @{ Kind = 'fn';     Name = 'compute_cert_hash' }
    @{ Kind = 'fn';     Name = 'parse_pin_from_fragment' }
)

# ---- logic：抽取 + mock，可在任意平台跑断言 ----
function New-LogicGen {
    param(
        [Parameter(Mandatory)][string]$RepoRoot,
        [Parameter(Mandatory)][string]$DevCheckRoot
    )
    $native = Join-Path $RepoRoot 'native'
    $genDir = Join-Path $DevCheckRoot 'rust/logic/src/gen'

    $staging = Read-RustSource -Path (Join-Path $native 'fs/staging.rs')
    $uninstall = Read-RustSource -Path (Join-Path $native 'installer/uninstall.rs')
    $plan = Read-RustSource -Path (Join-Path $native 'session/plan.rs')
    $pack = Read-RustSource -Path (Join-Path $native 'builder/pack.rs')
    $configKeys = Read-RustSource -Path (Join-Path $native 'utils/config_keys.rs')
    $secureTemp = Read-RustSource -Path (Join-Path $native 'utils/secure_temp.rs')
    $mirrorc = Read-RustSource -Path (Join-Path $native 'thirdparty/mirrorc.rs')
    $builderLocal = Read-RustSource -Path (Join-Path $native 'builder/local.rs')
    $embeddedName = Read-RustSource -Path (Join-Path $native 'embedded_name.rs')
    $builderExtract = Read-RustSource -Path (Join-Path $native 'builder/extract.rs')
    $h3 = Read-RustSource -Path (Join-Path $native 'capabilities/h3.rs')

    $parts = [System.Collections.Generic.List[string]]::new()
    $parts.Add(@'
// 生成物，勿手改：由 tools/devcheck/devcheck.ps1 按名字从 native/ 下抽取。
// 抽取规则见 tools/devcheck/lib/RustSource.ps1；找不到清单里的 item 会直接报错。
// 本文件被 src/main.rs 用 include! 展开到 crate 根，Path/PathBuf 由 main.rs 引入。

// ---- Windows 专有实现的跨平台桩（只影响这两个函数，其余全是仓库原码）----
// 桩的语义：路径里含 REPARSE 视为符号链接；is_under_system_root 用真实的 SystemRoot
// 判定（Linux 上 SystemRoot 未设置时退化成 C:\Windows，用例自己设置环境变量）。
fn has_reparse_point(path: &Path) -> bool {
    path.to_string_lossy().contains("REPARSE")
}
'@)

    $groups = @(
        @{ Items = $script:LogicStagingItems;      Src = $staging;      Name = 'fs/staging.rs' }
        @{ Items = $script:LogicUninstallItems;    Src = $uninstall;    Name = 'installer/uninstall.rs' }
        @{ Items = $script:LogicPlanItems;         Src = $plan;         Name = 'session/plan.rs' }
        @{ Items = $script:LogicPackItems;         Src = $pack;         Name = 'builder/pack.rs' }
        @{ Items = $script:LogicConfigKeysItems;   Src = $configKeys;   Name = 'utils/config_keys.rs' }
        @{ Items = $script:LogicSecureTempItems;   Src = $secureTemp;   Name = 'utils/secure_temp.rs' }
        @{ Items = $script:LogicMirrorcItems;      Src = $mirrorc;      Name = 'thirdparty/mirrorc.rs' }
        @{ Items = $script:LogicBuilderItems;      Src = $builderLocal; Name = 'builder/local.rs' }
        @{ Items = $script:LogicEmbeddedItems;     Src = $embeddedName; Name = 'embedded_name.rs' }
        @{ Items = $script:LogicExtractItems;      Src = $builderExtract; Name = 'builder/extract.rs' }
        @{ Items = $script:LogicH3Items;           Src = $h3;           Name = 'capabilities/h3.rs' }
    )

    foreach ($group in $groups) {
        $parts.Add("// ---- $($group.Name) ----")
        foreach ($item in $group.Items) {
            $parts.Add((Get-RustItem -Text $group.Src.Text -Masked $group.Src.Masked `
                        -Kind $item.Kind -Name $item.Name -SourceName $group.Name))
        }
    }

    $out = Join-Path $genDir 'extracted.rs'
    Write-GeneratedFile -Path $out -Content (($parts -join "`n`n") + "`n")
    return @($out)
}
