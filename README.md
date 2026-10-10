# Kirara

快速、多功能的通用安装程序与打包工具链。交付产物 `kirara-builder.exe` 是
「打包器 CLI + 安装器 GUI 模板」的拼接体，一个文件同时承担打包与安装两侧。

> 本仓库基于上游 [YuehaiTeam/kachina-installer](https://github.com/YuehaiTeam/kachina-installer)
> 的源码快照加本地修改；上游项目名、链接与来源说明保持不变，见
> [`UPSTREAM.md`](UPSTREAM.md) 与 [`LICENSE`](LICENSE)。与上游的差异（**无遥测**、
> 卸载范围收敛、下载后执行验签、依赖替换、旧版升级兼容等）逐项记录在
> [`MIGRATION_LEDGER.md`](MIGRATION_LEDGER.md)。

- 离线安装
  - 多线程安装，速度快
  - 安装后校验，避免错误
- 在线安装
  - 分块下载，边下载边解压
- 在线更新
  - 自动比对文件差异，增量更新
  - 支持`HDiffPatch`的文件级差分更新
  - 占用检测、结束进程
  - 只需要提供一个离线安装包链接即可完成以上所有操作，无需额外部署
- 运行库安装
  - 支持自动安装 .Net Runtime/Desktop 和 VCRedist
  - 自动安装Webview2以保证安装界面的正常运行
- 混合安装
  - 通过旧版安装包和在线更新直接安装最新版
- 卸载
  - 支持只删除包体内的文件，默认不删除用户数据
  - 品牌改名后能识别旧目录、旧进程与旧卸载器，升级与卸载都不留旧文件

## 使用方式

### 1. 取得 kirara-builder.exe

下游项目打包安装包时只需要这一个文件，不再检出本仓库源码。

- 直接取发布产物：从 [Releases](https://github.com/bainian-gudu/Kirara/releases)
  下载 `kirara-builder.exe`，并用同页的 `SHA256SUMS.txt` 校验
  （`sha256sum -c SHA256SUMS.txt`）。
- 自行构建（只在 Windows 上，需要 MSVC 与 WebView2 相关工具链）：

```powershell
pwsh build.ps1          # 产物 tools\kirara-builder.exe
pwsh build.ps1 -Force   # 已有产物也重新构建
```

完整子命令（`pack` / `gen` / `extract` / `replace-bin` / `append`）与参数以
`kirara-builder.exe --help` 为准。

### 2. 写配置文件

配置文件是 JSON（示例名 `kirara.config.json`，文件名任意）。上游字段的含义见下文示例，
本仓库另有几项上游没有的字段：

| 字段 | 作用 |
| --- | --- |
| `legacyExeNames` / `legacyUninstallNames` / `legacyProgramFilesPaths` | 品牌改名后的兼容：识别旧主程序名、旧卸载器名，以及注册表没有记录时按序兜底探测的旧默认安装目录（相对 `%ProgramFiles%`） |
| `extraUninstallLnkNames` | 卸载时清理的快捷方式文件名；用户/公共桌面与用户/公共开始菜单四处都会试 |
| `extraUninstallRegistry` | 卸载时清理宿主自己写过的注册表项（`hive` / `key` / `value`）；不带 `value` 时只能指向键路径里含 `regName` 整段的产品键，带 `value` 时共享容器也只删该值且值名须以 `regName` 开头；`hive: HKCU` 会同时遍历已加载的其他用户配置单元 |
| `extraUninstallScheduledTasks` | 卸载时清理的登录计划任务名；必须带 `regName` 前缀，通配符一律拒绝 |
| `agreementFile` / `agreementFormat` / `agreementTitle` | 打包期内联用户协议正文；三个字段见「用户协议（打包期内联）」一节 |
| `imageFile` / `iconFile` | 打包期内联左栏图片与程序图标；两个字段见「左栏图片与程序图标」一节 |
| `userDataPath` | 卸载勾选后清理的用户数据目录；`%VAR%` 会展开，并按已加载的用户配置单元重放，所以卸载器提权运行时也能清掉当初普通用户那份数据 |

```jsonc
{
  // 离线包下载地址，需要固定
  "source": "https://example.com/Kirara.Install.exe",
  // 注册表中的应用名称
  "appName": "Kirara",
  // 注册表中的发布者
  "publisher": "bainian-gudu",
  // 注册表中的应用ID
  "regName": "Kirara",
  // 主程序文件名
  "exeName": "Kirara.exe",
  // 卸载程序文件名
  "uninstallName": "Kirara.uninst.exe",
  // 更新器文件名
  "updaterName": "Kirara.update.exe",
  // 默认安装路径，和Program Files相对
  "programFilesPath": "Kirara",
  // GUI里的标题
  "title": "Kirara",
  // GUI里的副标题
  "description": "快速多功能的安装器",
  // 窗口标题
  "windowTitle": "Kirara 安装程序",
  // 卸载时需要删除的用户数据目录或文件
  "userDataPath": ["${INSTALL_PATH}/User"],
  // 更新时如果文件夹已存在且非空则跳过的目录
  "ignoreFolderPath": ["${INSTALL_PATH}/cache"],
  // 卸载时需要额外删除的其他目录或文件
  "extraUninstallPath": ["${INSTALL_PATH}/log"],
  // UAC 策略
  // prefer-admin: 除非用户安装在%User%、%AppData%、%Documents%、%Desktop%、%Downloads%目录，都请求UAC
  // prefer-user: 只在用户没有权限写入的目录请求UAC
  // force: 强制请求UAC
  "uacStrategy": "prefer-admin",
  // 需要安装的运行库，以下为目前支持的列表
  "runtimes": [
    // .NET 的版本号支持 8/8.0/8.0.13 的格式
    "Microsoft.DotNet.DesktopRuntime.8",
    "Microsoft.DotNet.Runtime.8",
    // VCRedist 只支持以下两种格式
    "Microsoft.VCRedist.2015+.x64",
    "Microsoft.VCRedist.2015+.x86",
  ],
}
```

### 3. 左栏图片与程序图标

安装界面左栏的图片与输出 exe 的图标都由配置指定，两个字段都不写进包内配置：

| 字段 | 作用 |
| --- | --- |
| `imageFile` | 左栏图片，相对配置文件所在目录解析；内容识别为 WebP 时作图片，纯文本按 CSS 主题注入 |
| `iconFile` | 输出 exe 的图标（`.ico`），相对配置文件所在目录解析 |

```jsonc
{
  "imageFile": "left.webp",
  "iconFile": "app.ico"
}
```

两个字段都不配置时，左栏用内置图、exe 用内置图标；配置了却读不到文件时打包报错。
`pack` 的 `--image`（`-t`）与 `--icon` 优先于配置，指向的文件不存在同样让打包失败。
图标对三个 exe 都生效：安装器与更新器各自 `pack` 时写进映像，卸载器在安装期由安装器
（更新时由更新器）的映像复制而来。三步 `pack` 共用同一份配置，界面与图标因此一致；
改用命令行参数时三步要传同一份参数，否则安装器自更新会把界面换回不带自定义资源的那份。

### 4. 用户协议（打包期内联）

安装包是单文件 exe，运行期没有仓库上下文，协议正文在打包期读入并写进包内配置。这三个字段
上游没有：

| 字段 | 作用 |
| --- | --- |
| `agreementFile` | 协议正文文件，相对配置文件所在目录解析；不配置就没有同意门槛 |
| `agreementFormat` | 正文格式：`markdown` / `md` 渲染 Markdown，`html` 渲染 HTML，其余值按纯文本 |
| `agreementTitle` | 协议标题，缺省「用户协议」 |

```jsonc
{
  "agreementFile": "USER_AGREEMENT.txt",
  "agreementFormat": "markdown",
  "agreementTitle": "用户协议"
}
```

`pack` 把正文读进包内配置的 `agreement: { title, format, content }`，三个源字段不再出现在
包内配置里；正文的 BOM 会去掉、换行统一成 LF。文件读不到只警告、不中断打包，此时包内没有
协议正文，界面上的协议入口退化为不可点击的文字。内联了正文的安装包，安装时必须先勾选同意
才能继续。

### 5. 打包三步

1. 构建更新器，用于打包在便携版内等。更新器不需要被打包到离线包内。

```bat
kirara-builder.exe pack -c kirara.config.json -o Kirara.update.exe
```

可选：为输出的 exe 设置图标和自定义 css / 左侧图片：

```bat
kirara-builder.exe pack -c kirara.config.json -o Kirara.update.exe --icon icon.ico -t custom.webp
```

这两项写进配置（见「左栏图片与程序图标」一节）时三步共用一份，不必逐条传参。

2. 构建Metadata、压缩应用文件

```bat
kirara-builder.exe gen -j 8 -i {AppDir} -m metadata.json -o hashed -r {AppId} -t {Version} -u Kirara.update.exe
```

3. 构建离线包

```bat
kirara-builder.exe pack -c kirara.config.json -m metadata.json -d hashed -o Kirara.Install.exe
```

4. 部署离线包到服务器上，确保可以通过json里的url下载到。在目前版本里，你不需要部署压缩产生的`hashed`文件夹和metadata文件，这些文件是在构建过程中临时使用的。
5. 此时第一步得到的更新器可以直接作为在线安装包使用。

### 6. 静默安装、卸载与指定目录

安装器接受下列开关，安装与卸载都适用：

```bat
Kirara.Install.exe -S              :: 静默安装（/S、/VERYSILENT 等价）
Kirara.Install.exe -I              :: 非交互：不询问，界面照常显示
Kirara.Install.exe -S -D D:\App    :: 指定安装目录
Kirara.uninst.exe  -S              :: 静默卸载
```

### 7. 查看/提取离线包内容（kirara-builder extract）

用于调试/排查打包结果：

- 列出离线包内嵌资源（会同时展示 hash 名称与 metadata 中的原始文件名）：

```bat
kirara-builder.exe extract -i Kirara.Install.exe --list
```

- 解包全部文件到目录（会尽量使用 metadata 的原始文件路径；若无 metadata 则输出 hash 名）：

```bat
kirara-builder.exe extract -i Kirara.Install.exe --all out_dir
```

- 按 metadata 文件名提取指定文件（需要离线包内含 metadata）：

```bat
kirara-builder.exe extract -i Kirara.Install.exe --meta-name "Main.exe"
```

提示：`--name` / `--meta-name` / `--all` / `--list` 四种模式互斥，一次只能用一种。

### 8. 多安装源

如果你希望用户可以自由选择安装源，你可以指定多个Source，此时用户主动打开安装器时将在路径选择上方看到安装源选择按钮。

示例配置如下：

```
{
  "source": [
    {
      "id": "stable",
      "name": "正式版",
      "uri": "https://example.com/Kirara.Install.exe"
    },
    {
      "id": "beta",
      "name": "测试版",
      "uri": "https://example.com/Kirara.Install.Beta.exe"
    }
  ]
}
```

### 9. Mirror酱平台支持

[Mirror酱](https://mirrorchyan.com) 是独立的第三方软件下载平台，提供付费的软件下载加速服务。本安装器接入了Mirror酱的API，允许用户使用Mirror酱更新软件。例如，你可以结合上述的安装源选择功能，让用户选择使用自建服务器更新还是使用Mirror酱更新。

如需使用，请设置`source`的值为`mirrorc://{rid}?channel={stable|beta|alpha}`。同时，你需要将前述产生的`.metadata.json`放置到上传给Mirror酱的文件中。示例的上传格式：

```
upload_to_mirrorc.zip
 - .metadata.json
 - Main.exe
 - Main.update.exe
```

也支持

```
upload_to_mirrorc.zip
 - App/.metadata.json
 - App/Main.exe
 - App/Main.update.exe
```

Tips：Mirror酱使用独立的文件级增量更新机制，因此当选择Mirror酱作为更新源时候，将无法使用安装器自带的版本比对、二进制Patch级增量等功能。

## 部分技术细节

安装器的离线包是一个可寻址的文件，其中包含了安装器主体、索引、配置、元数据、程序文件、Patch文件。当安装程序运行时，如果程序没有有内嵌资源，会对配置URL中的离线包进行远程寻址，通过文件头中的索引获取资源，并通过HTTP 206 部分下载需要的内容。如果程序有内嵌资源，程序会对比线上和本地的版本，优先使用本地的资源，并在可行的情况下使用先释放本地资源、随后使用服务器上的更新Patch的形式以减少流量损耗。

安装程序和dfs服务器不是强绑定关系，任何可以通过HTTP提供离线包下载的服务器都可以作为更新服务器。dfs在本项目中仅作为一个获取下载地址的API使用。

上游的技术细节可以看看 [![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/YuehaiTeam/kachina-installer) 。

本仓库自己的快速体检（不跑完整构建的源码快照 / 安全阀 / 前端 / CI 不变量检查，含故障注入自检）见 `tools/devcheck/README.md`：`pwsh tools/devcheck/devcheck.ps1`。
