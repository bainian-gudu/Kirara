// P6 端到端重演：冻结旧安装包 → 固定 SHA 的 builder 重建新包 → 升级 → 卸载，
// 并在隔离目录跑一次下游的 pack → gen → pack。
//
// 输入全部来自环境变量（CI 的 p6-e2e.yml 提供）：
//   P6_BUILDER              固定 SHA 的 kirara-builder.exe（拼接体）
//   P6_OLD_URL/P6_OLD_SHA256 冻结旧安装包与它的校验和
//   P6_DOWNSTREAM_REPO/REF  下游配置来源（packaging/packaging.config.json 与根 USER_AGREEMENT.txt）
//   P6_NEW_VERSION          新包版本号，需高于旧包才会走升级
//
// 断言的是两条合同：旧包留下的现场（安装目录、ARP 记录、快捷方式、用户数据、
// 自启项与计划任务）能被新安装器接住、被新卸载器收干净；下游配置能被 builder
// 的 pack → gen → pack 原样接受。

import {
  getTestDir,
  FLAGS,
  runInstaller,
  assertExitOk,
  readUninstallRecord,
  getFileHash,
  assertStagingRemoved,
  clearLogFile,
  getLogFilePath,
} from './utils.mjs';
import 'zx/globals';
import { usePwsh } from 'zx';
import os from 'os';
usePwsh();

const BUILDER = process.env.P6_BUILDER ?? '';
const OLD_URL = process.env.P6_OLD_URL ?? '';
const OLD_SHA = process.env.P6_OLD_SHA256 ?? '';
const DOWNSTREAM_REPO = process.env.P6_DOWNSTREAM_REPO ?? '';
const DOWNSTREAM_REF = process.env.P6_DOWNSTREAM_REF ?? '';
const NEW_VERSION = process.env.P6_NEW_VERSION ?? '';

const APP = 'HoYoEnhance';
const REG_NAME = 'HoYoEnhance';
const TASK_NAME = 'HoYoEnhance.AutoStart';
const MARKER = 'p6-marker.txt';

const failures = [];

function check(ok, label, detail = '') {
  const suffix = detail ? ` (${detail})` : '';
  if (ok) {
    console.log(chalk.green(`  ✓ ${label}`));
  } else {
    console.log(chalk.red(`  ✗ ${label}${suffix}`));
    failures.push(`${label}${suffix}`);
  }
}

function step(title) {
  console.log(chalk.blue(`\n=== ${title} ===`));
}

async function download(url, dest) {
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok) {
    throw new Error(`下载失败 ${url}: HTTP ${res.status}`);
  }
  await fs.writeFile(dest, Buffer.from(await res.arrayBuffer()));
}

async function findUninstallRecord() {
  for (const hive of ['HKCU', 'HKLM']) {
    const record = await readUninstallRecord(hive, REG_NAME);
    if (record) {
      return { hive, record };
    }
  }
  return null;
}

async function runTask(args) {
  const result = await $({ reject: false })`schtasks ${args}`.quiet();
  return result.exitCode === 0;
}

async function taskExists() {
  return runTask(['/Query', '/TN', TASK_NAME]);
}

async function runValue() {
  const out =
    await $`(Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name ${REG_NAME} -ErrorAction SilentlyContinue).${REG_NAME}`.quiet();
  return out.stdout.trim();
}

async function userShortcutPaths() {
  const desktop = (
    await $`[Environment]::GetFolderPath('Desktop')`.quiet()
  ).stdout.trim();
  const programs = (
    await $`[Environment]::GetFolderPath('Programs')`.quiet()
  ).stdout.trim();
  return {
    desktop: path.join(desktop, `${APP}.lnk`),
    programs: path.join(programs, APP),
  };
}

async function unpack(args, cwd) {
  const run = cwd ? $({ cwd }) : $;
  const result = await run`& ${BUILDER} ${args}`.quiet();
  if (result.exitCode !== 0) {
    throw new Error(
      `builder ${args.join(' ')} 失败（exit ${result.exitCode}）\n${result.stderr}`,
    );
  }
  return result.stdout;
}

async function main() {
  for (const [name, value] of Object.entries({
    P6_BUILDER: BUILDER,
    P6_OLD_URL: OLD_URL,
    P6_OLD_SHA256: OLD_SHA,
    P6_DOWNSTREAM_REPO: DOWNSTREAM_REPO,
    P6_DOWNSTREAM_REF: DOWNSTREAM_REF,
    P6_NEW_VERSION: NEW_VERSION,
  })) {
    if (!value) {
      throw new Error(`缺少环境变量 ${name}`);
    }
  }
  if (!(await fs.pathExists(BUILDER))) {
    throw new Error(`builder 不存在：${BUILDER}`);
  }

  const work = process.env.P6_WORK ?? getTestDir('p6-e2e');
  const installDir = path.join(work, 'install');
  const payloadDir = path.join(work, 'payload');
  const downstreamDir = path.join(work, 'downstream');
  const packDir = path.join(work, 'pack');
  const outDir = path.join(work, 'out');
  const oldPackage = path.join(work, 'old-install.exe');
  await fs.ensureDir(work);
  await fs.ensureDir(packDir);
  await fs.ensureDir(outDir);

  step('固定产物');
  const builderHash = await getFileHash(BUILDER);
  console.log(`  builder  ${BUILDER}\n           ${builderHash}`);

  await download(OLD_URL, oldPackage);
  const oldHash = await getFileHash(oldPackage);
  check(oldHash === OLD_SHA, '旧安装包 SHA256 与冻结值一致', oldHash);
  console.log(`  旧包     ${OLD_URL}`);

  for (const rel of ['packaging/packaging.config.json', 'USER_AGREEMENT.txt']) {
    const dest = path.join(downstreamDir, rel);
    await fs.ensureDir(path.dirname(dest));
    await download(
      `https://raw.githubusercontent.com/${DOWNSTREAM_REPO}/${DOWNSTREAM_REF}/${rel}`,
      dest,
    );
  }
  const config = path.join(downstreamDir, 'packaging/packaging.config.json');
  console.log(`  下游配置 ${DOWNSTREAM_REPO}@${DOWNSTREAM_REF}`);

  step('隔离打包：extract → pack → gen → pack');
  const listed = await unpack(['extract', '--list', '-i', oldPackage]);
  check(
    listed.includes('CONFIG') && listed.includes('INDEX') && listed.includes(`${APP}.exe`),
    '旧包可以列出载荷清单',
  );
  await unpack(['extract', '--all', payloadDir, '-i', oldPackage]);
  // extract --all 按 metadata 里的相对路径落盘，载荷根就是暂存目录
  const appDir = payloadDir;
  check(await fs.pathExists(path.join(appDir, `${APP}.exe`)), '载荷按真实路径落盘');

  // 与下游 pack.ps1 的暂存口径一致：图片不进包，协议正文随包内联。
  for (const rel of ['USER_AGREEMENT.txt', 'LICENSE', 'config.example.json']) {
    const src = path.join(downstreamDir, rel);
    if (await fs.pathExists(src)) {
      await fs.copy(src, path.join(appDir, rel));
    }
  }
  await fs.remove(path.join(appDir, `${APP}.update.exe`));
  await unpack(['pack', '-c', config, '-o', path.join(appDir, `${APP}.update.exe`)]);
  const updaterHash = await getFileHash(path.join(appDir, `${APP}.update.exe`));

  await unpack(
    [
      'gen',
      '-j',
      '6',
      '-i',
      appDir,
      '-m',
      path.join(packDir, 'metadata.json'),
      '-o',
      path.join(packDir, 'hashed'),
      '-r',
      DOWNSTREAM_REPO,
      '-t',
      NEW_VERSION,
      '-u',
      path.join(appDir, `${APP}.update.exe`),
    ],
    packDir,
  );
  const newPackage = path.join(outDir, `${APP}.Install.${NEW_VERSION}.exe`);
  await unpack(
    [
      'pack',
      '-c',
      config,
      '-m',
      path.join(packDir, 'metadata.json'),
      '-d',
      path.join(packDir, 'hashed'),
      '-o',
      newPackage,
    ],
    packDir,
  );
  check(await fs.pathExists(newPackage), '隔离目录里产出新安装包');
  const newListed = await unpack(['extract', '--list', '-i', newPackage]);
  check(
    newListed.includes('INDEX') && newListed.includes(`${APP}.exe`),
    '新包载荷清单含索引与主程序',
  );

  step('旧包装入');
  await clearLogFile();
  assertExitOk(
    await runInstaller(oldPackage, [FLAGS, '-D', installDir], '旧包安装', '10m'),
    '旧包安装',
  );
  for (const name of [`${APP}.exe`, `${APP}.uninst.exe`, `${APP}.update.exe`]) {
    check(await fs.pathExists(path.join(installDir, name)), `旧包装出 ${name}`);
  }
  const installed = await findUninstallRecord();
  check(installed !== null, '旧包写下 ARP 卸载记录');
  const shortcuts = await userShortcutPaths();
  check(
    await fs.pathExists(shortcuts.desktop),
    '旧包装出桌面快捷方式',
    shortcuts.desktop,
  );

  // 旧版应用运行期才会写的东西：卸载器必须能按新配置回收。
  const dataDir = path.join(
    process.env.LOCALAPPDATA ?? '',
    APP,
  );
  await fs.ensureDir(dataDir);
  await fs.writeFile(path.join(dataDir, MARKER), 'p6\n');
  await $`New-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name ${REG_NAME} -Value ${path.join(installDir, `${APP}.exe`)} -PropertyType String -Force | Out-Null`.quiet();
  await runTask(['/Create', '/TN', TASK_NAME, '/TR', path.join(installDir, `${APP}.exe`), '/SC', 'ONLOGON', '/F']);
  check((await runValue()) !== '', '自启项与计划任务已布置', TASK_NAME);

  step('升级到新包');
  assertExitOk(
    await runInstaller(newPackage, [FLAGS, '-D', installDir], '新包升级', '10m'),
    '新包升级',
  );
  const upgraded = await findUninstallRecord();
  check(
    upgraded?.record?.DisplayVersion === NEW_VERSION,
    'ARP 记录版本被升级改写',
    upgraded?.record?.DisplayVersion ?? '无记录',
  );
  check(
    (await getFileHash(path.join(installDir, `${APP}.update.exe`))) === updaterHash,
    '安装目录里的更新器换成了新 builder 的产物',
  );
  check(
    await fs.pathExists(path.join(dataDir, MARKER)),
    '升级不动用户数据',
  );

  step('卸载');
  assertExitOk(
    await runInstaller(
      path.join(installDir, `${APP}.uninst.exe`),
      [FLAGS],
      '新卸载器',
      '10m',
    ),
    '新卸载器',
  );
  check(!(await fs.pathExists(installDir)), '安装目录被删除');
  check((await findUninstallRecord()) === null, 'ARP 记录被删除');
  check(
    !(await fs.pathExists(shortcuts.desktop)),
    '桌面快捷方式被删除',
  );
  check((await runValue()) === '', '自启项被回收');
  check(!(await taskExists()), '计划任务被回收');
  check(
    await fs.pathExists(path.join(dataDir, MARKER)),
    '未勾选时用户数据保留',
  );
  try {
    await assertStagingRemoved(installDir);
    check(true, '暂存目录已回收');
  } catch (error) {
    check(false, '暂存目录已回收', error.message);
  }

  const temp = os.tmpdir();
  const leftovers = (await fs.readdir(temp)).filter(
    (name) =>
      name.startsWith('kachina.uninst.') ||
      name.startsWith('Kachina.RuntimePackage.'),
  );
  check(leftovers.length === 0, '%TEMP% 里安装期临时文件已回收', leftovers.join(', '));
  await fs.remove(dataDir);

  if (failures.length > 0) {
    console.error(chalk.red(`\n${failures.length} 项断言失败：`));
    for (const failure of failures) {
      console.error(chalk.red(`  - ${failure}`));
    }
    if (await fs.pathExists(getLogFilePath())) {
      console.error(chalk.yellow('\n=== 安装器日志尾部 ==='));
      const log = await fs.readFile(getLogFilePath(), 'utf-8');
      console.error(log.split('\n').slice(-60).join('\n'));
    }
    process.exitCode = 1;
  } else {
    console.log(chalk.green('\n✓ P6 端到端全部通过'));
  }

  await fs.writeJSON(path.join(work, 'p6-summary.json'), {
    builder: { path: BUILDER, sha256: builderHash },
    oldPackage: { url: OLD_URL, sha256: oldHash },
    newPackage: { path: newPackage, version: NEW_VERSION },
    downstream: { repo: DOWNSTREAM_REPO, ref: DOWNSTREAM_REF },
    failures,
  });
  if (await fs.pathExists(getLogFilePath())) {
    await fs.copy(getLogFilePath(), path.join(work, 'KachinaInstaller.log'));
  }
  console.log(chalk.gray(`\n现场保留在 ${work}`));
}

main().catch(async (error) => {
  console.error(chalk.red('P6 重演中断：'), error.message);
  if (await fs.pathExists(getLogFilePath())) {
    console.error(chalk.yellow('\n=== 安装器日志尾部 ==='));
    const log = await fs.readFile(getLogFilePath(), 'utf-8');
    console.error(log.split('\n').slice(-60).join('\n'));
  }
  process.exit(1);
});
