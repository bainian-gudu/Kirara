import {
  cleanupTestDir,
  getTestDir,
  runInstaller,
  assertExitOk,
  clearLogFile,
} from './utils.mjs';
import 'zx/globals';
import { usePwsh } from 'zx';
usePwsh();

// dump 由会话直接写出，静默会话就够；`-I` 在 debug 构建下会拉起指向 rsbuild
// dev server 的 UI 窗口，跑 debug 产物的环境（CI）没有它，所以固定用 `-S`。
const FLAGS_SILENT = '-S';

async function test() {
  const testDir = getTestDir('dump-offline-install');
  const dumpDir = path.resolve('./plan-dumps/offline-install');
  const installerPath = './fixtures/test-app-v1.exe';

  console.log(chalk.blue('=== Dump offline install plan ==='));
  await fs.emptyDir(dumpDir);

  try {
    await clearLogFile();
    const result = await runInstaller(
      installerPath,
      [FLAGS_SILENT, '-D', testDir, '--dump-dir', dumpDir],
      'Offline install dump',
    );
    assertExitOk(result, 'Offline install dump');
    for (const name of [
      '01-settings.json',
      '02-meta-scan.json',
      '03-plan.json',
    ]) {
      if (!(await fs.pathExists(path.join(dumpDir, name)))) {
        throw new Error(`missing dump ${name}`);
      }
    }
    console.log(chalk.green(`✓ Dumps written to ${dumpDir}`));
  } catch (error) {
    console.error(chalk.red('Test failed:'), error.message);
    process.exit(1);
  } finally {
    await cleanupTestDir(testDir);
  }
  process.exit(0);
}

test();
